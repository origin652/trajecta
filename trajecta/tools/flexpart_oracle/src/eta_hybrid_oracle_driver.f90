! SPDX-License-Identifier: GPL-3.0-or-later
!
! Minimal external FLEXPART oracle driver for ERA5 hybrid-137 ETA meteorology.
! This program is intentionally GPL-facing: it links the frozen FLEXPART
! modules and calls their real vertical transform and interpolation routines.

program eta_hybrid_oracle_driver
  use, intrinsic :: iso_fortran_env, only: int64, error_unit
  use, intrinsic :: ieee_arithmetic, only: ieee_is_finite
  use netcdf
  use com_mod
  use windfields_mod
  use particle_mod, only: part
  use interpol_mod, only: alloc_interpol, init_interpol, hor_interpol, &
    interpol_wind, interpol_partoutput_val, interpol_pbl, interpol_pbl_short, &
    dz1out, fp_u => u, fp_v => v, fp_w => w
  use verttransform_mod, only: verttransform_ecmwf
  use coord_ecmwf_mod, only: z_to_zeta
  use oracle_calcpar_mod, only: oracle_calcpar, set_oracle_pbl_height_mode, &
    PBL_HEIGHT_RICHARDSON_DIAGNOSED, oracle_pbl_height_mode
  use class_gribfile_mod, only: GRIBFILE_CENTRE_NCEP, GRIBFILE_CENTRE_ECMWF
  use par_mod, only: ga, pi, r_earth, r_air
  use qvsat_mod, only: ew
  implicit none

  integer, parameter :: native_anchor = 1
  integer, parameter :: interpolated_common = 2
  integer, parameter :: surface_layer = 3
  integer, parameter :: modern_difference = 4
  integer, parameter :: vertical_agl = 1
  integer, parameter :: vertical_asl = 2
  integer, parameter :: vertical_pressure = 3
  integer, parameter :: point_id_len = 192
  real, parameter :: common_g0 = 9.80665
  real, parameter :: common_earth_radius = 6371229.0

  character(len=1024) :: pressure_path, surface_path, query_path, output_path
  integer :: nx_in, ny_in, nlev_in, nt_in
  integer(int64), allocatable :: valid_times(:)
  real, allocatable :: longitude(:), latitude_desc(:), pressure_hpa(:)
  real, allocatable :: ap_half(:), b_half(:), lnsp_data(:,:,:)
  real, allocatable :: u_data(:,:,:,:), v_data(:,:,:,:), t_data(:,:,:,:)
  real, allocatable :: q_data(:,:,:,:), omega_data(:,:,:,:), z_data(:,:,:,:)
  real, allocatable :: sp_data(:,:,:), surface_z_data(:,:,:)
  real, allocatable :: t2m_data(:,:,:), d2m_data(:,:,:)
  real, allocatable :: u10_data(:,:,:), v10_data(:,:,:)
  real, allocatable :: blh_data(:,:,:), ishf_data(:,:,:), zust_data(:,:,:)
  real, allocatable :: fsr_data(:,:,:), ie_data(:,:,:)
  logical :: have_pbl_inputs = .false.
  logical :: pbl_has_ishf = .false., pbl_has_ustar = .false., pbl_has_blh = .false.
  character(len=16) :: source_family = 'era5'
  character(len=16) :: vertical_coordinate = 'hybrid_eta'
  character(len=32) :: pbl_height_mode = 'richardson_diagnosed'
  real, allocatable :: pv_raw(:,:,:)
  integer :: loaded_left = 0, loaded_right = 0
  integer :: k_iface

  call read_arguments
  call read_pressure_file
  call read_surface_file
  call initialise_flexpart_state
  call run_queries

contains

  subroutine read_arguments
    if (command_argument_count() /= 4) then
      write(error_unit,'(A)') 'usage: eta_hybrid_oracle_driver HYBRID.nc SURFACE.nc QUERIES.txt OUTPUT.tsv'
      error stop 2
    end if
    call get_command_argument(1, pressure_path)
    call get_command_argument(2, surface_path)
    call get_command_argument(3, query_path)
    call get_command_argument(4, output_path)
  end subroutine read_arguments

  subroutine read_pressure_file
    integer :: ncid, dimid, varid
    real, allocatable :: scratch4(:,:,:,:)

    call check_nc(nf90_open(trim(pressure_path), nf90_nowrite, ncid), 'open pressure file')
    call check_nc(nf90_inq_dimid(ncid, 'longitude', dimid), 'longitude dimension')
    call check_nc(nf90_inquire_dimension(ncid, dimid, len=nx_in), 'longitude length')
    call check_nc(nf90_inq_dimid(ncid, 'latitude', dimid), 'latitude dimension')
    call check_nc(nf90_inquire_dimension(ncid, dimid, len=ny_in), 'latitude length')
    call check_nc(nf90_inq_dimid(ncid, 'hybrid', dimid), 'hybrid dimension')
    call check_nc(nf90_inquire_dimension(ncid, dimid, len=nlev_in), 'hybrid length')
    call check_nc(nf90_inq_dimid(ncid, 'valid_time', dimid), 'time dimension')
    call check_nc(nf90_inquire_dimension(ncid, dimid, len=nt_in), 'time length')

    if (nx_in < 2 .or. ny_in < 2 .or. nlev_in < 3 .or. nt_in < 2) then
      write(error_unit,'(A,4(1X,I0))') 'invalid pressure dimensions', nx_in, ny_in, nlev_in, nt_in
      error stop 3
    end if
    if (numwfmem < 2) then
      write(error_unit,'(A,I0)') 'FLEXPART numwfmem must be at least two, got ', numwfmem
      error stop 3
    end if

    allocate(longitude(nx_in), latitude_desc(ny_in), pressure_hpa(nlev_in), valid_times(nt_in))
    allocate(ap_half(nlev_in+1), b_half(nlev_in+1), lnsp_data(nx_in,ny_in,nt_in))
    call read_real_1d(ncid, 'longitude', longitude)
    call read_real_1d(ncid, 'latitude', latitude_desc)
    call read_i64_1d(ncid, 'valid_time', valid_times)
    call read_real_1d(ncid, 'ap', ap_half)
    call read_real_1d(ncid, 'b', b_half)
    call assert_strictly_increasing(longitude, 'longitude')
    call assert_strictly_decreasing(latitude_desc, 'latitude')
    call assert_strictly_increasing_i64(valid_times, 'valid_time')
    do k_iface = 1, nlev_in
      pressure_hpa(k_iface) = 0.5*(ap_half(k_iface)+ap_half(k_iface+1))/100.0
    end do

    allocate(scratch4(nx_in,ny_in,nlev_in,nt_in))
    allocate(u_data(nx_in,ny_in,nlev_in,nt_in), v_data(nx_in,ny_in,nlev_in,nt_in))
    allocate(t_data(nx_in,ny_in,nlev_in,nt_in), q_data(nx_in,ny_in,nlev_in,nt_in))
    allocate(omega_data(nx_in,ny_in,nlev_in,nt_in), z_data(nx_in,ny_in,nlev_in,nt_in))

    call read_real_4d(ncid, 'u', scratch4); call reverse_latitude_4d(scratch4, u_data)
    call read_real_4d(ncid, 'v', scratch4); call reverse_latitude_4d(scratch4, v_data)
    call read_real_4d(ncid, 't', scratch4); call reverse_latitude_4d(scratch4, t_data)
    call read_real_4d(ncid, 'q', scratch4); call reverse_latitude_4d(scratch4, q_data)
    call read_real_4d(ncid, 'w', scratch4); call reverse_latitude_4d(scratch4, omega_data)
    z_data = 0.0
    block
      real, allocatable :: scratch3(:,:,:)
      allocate(scratch3(nx_in,ny_in,nt_in))
      call read_real_3d(ncid, 'lnsp', scratch3)
      call reverse_latitude_3d(scratch3, lnsp_data)
      deallocate(scratch3)
    end block
    deallocate(scratch4)
    call check_nc(nf90_close(ncid), 'close hybrid file')
  end subroutine read_pressure_file

  subroutine read_surface_file
    integer :: ncid, dimid, nxs, nys, nts
    integer(int64), allocatable :: surface_times(:)
    real, allocatable :: scratch3(:,:,:), surface_lon(:), surface_lat(:)

    call check_nc(nf90_open(trim(surface_path), nf90_nowrite, ncid), 'open surface file')
    call check_nc(nf90_inq_dimid(ncid, 'longitude', dimid), 'surface longitude dimension')
    call check_nc(nf90_inquire_dimension(ncid, dimid, len=nxs), 'surface longitude length')
    call check_nc(nf90_inq_dimid(ncid, 'latitude', dimid), 'surface latitude dimension')
    call check_nc(nf90_inquire_dimension(ncid, dimid, len=nys), 'surface latitude length')
    call check_nc(nf90_inq_dimid(ncid, 'valid_time', dimid), 'surface time dimension')
    call check_nc(nf90_inquire_dimension(ncid, dimid, len=nts), 'surface time length')
    if (nxs /= nx_in .or. nys /= ny_in .or. nts /= nt_in) then
      write(error_unit,'(A)') 'surface grid/time shape differs from pressure file'
      error stop 4
    end if

    allocate(surface_times(nt_in), surface_lon(nx_in), surface_lat(ny_in))
    call read_i64_1d(ncid, 'valid_time', surface_times)
    call read_real_1d(ncid, 'longitude', surface_lon)
    call read_real_1d(ncid, 'latitude', surface_lat)
    if (any(surface_times /= valid_times) .or. any(surface_lon /= longitude) .or. &
        any(surface_lat /= latitude_desc)) then
      write(error_unit,'(A)') 'surface coordinates differ from pressure file'
      error stop 4
    end if

    allocate(scratch3(nx_in,ny_in,nt_in))
    allocate(sp_data(nx_in,ny_in,nt_in), surface_z_data(nx_in,ny_in,nt_in))
    allocate(t2m_data(nx_in,ny_in,nt_in), d2m_data(nx_in,ny_in,nt_in))
    allocate(u10_data(nx_in,ny_in,nt_in), v10_data(nx_in,ny_in,nt_in))
    allocate(blh_data(nx_in,ny_in,nt_in), ishf_data(nx_in,ny_in,nt_in), zust_data(nx_in,ny_in,nt_in))
    allocate(fsr_data(nx_in,ny_in,nt_in), ie_data(nx_in,ny_in,nt_in))
    blh_data = -1.0; ishf_data = 0.0; zust_data = -1.0; fsr_data = 0.0; ie_data = 0.0
    pbl_has_ishf = .false.; pbl_has_ustar = .false.; pbl_has_blh = .false.
    ! Hybrid surface product may omit sp; surface pressure comes from exp(lnsp).
    sp_data = 0.0
    call read_real_3d(ncid, 'z', scratch3); call reverse_latitude_3d(scratch3, surface_z_data)
    call read_real_3d(ncid, 't2m', scratch3); call reverse_latitude_3d(scratch3, t2m_data)
    call read_real_3d(ncid, 'd2m', scratch3); call reverse_latitude_3d(scratch3, d2m_data)
    call read_real_3d(ncid, 'u10', scratch3); call reverse_latitude_3d(scratch3, u10_data)
    call read_real_3d(ncid, 'v10', scratch3); call reverse_latitude_3d(scratch3, v10_data)
    call try_read_pbl_field(ncid, 'blh', scratch3, blh_data, pbl_has_blh, -1.0)
    call try_read_pbl_field(ncid, 'ishf', scratch3, ishf_data, pbl_has_ishf, 0.0)
    call try_read_pbl_field(ncid, 'zust', scratch3, zust_data, pbl_has_ustar, -1.0)
    call try_read_real_3d(ncid, 'fsr', scratch3, fsr_data)
    call try_read_real_3d(ncid, 'ie', scratch3, ie_data)
    call set_oracle_identity_from_env
    call validate_pbl_inputs
    deallocate(scratch3, surface_times, surface_lon, surface_lat)
    call check_nc(nf90_close(ncid), 'close surface file')
  end subroutine read_surface_file

  subroutine initialise_flexpart_state
    integer :: k

    nx = nx_in; ny = ny_in; nxfield = nx_in
    nxmax = nx_in; nymax = ny_in
    nxmin1 = nx_in - 1; nymin1 = ny_in - 1
    ! +1 artificial ground level following FLEXPART ECMWF hybrid setup.
    nuvz = nlev_in + 1; nwz = nlev_in + 1; nz = nlev_in + 1
    nuvzmax = nlev_in + 1; nwzmax = nlev_in + 1; nzmax = nlev_in + 1
    nconvlevmax = nlev_in + 1; na = nlev_in + 1
    xlon0 = longitude(1)
    ylat0 = latitude_desc(ny_in)
    dx = longitude(2) - longitude(1)
    dy = latitude_desc(ny_in-1) - latitude_desc(ny_in)
    dxconst = 180.0 / (dx * r_earth * pi)
    dyconst = 180.0 / (dy * r_earth * pi)
    xglobal = .false.; nglobal = .false.; sglobal = .false.
    switchnorthg = 999999.0; switchsouthg = -999999.0
    numbnests = 0; nxmaxn = 1; nymaxn = 1
    numthreads = 1; numthreads_grid = 1
    ipin = 0; loutrestart = -1
    lcw = .false.; lcwsum = .false.
    DRYDEP = .false.; WETDEP = .false.; DEP = .false.
    lsubgrid = 0
    ! metdata_format already fixed during surface PBL validation

    call alloc_fixedfields
    call alloc_windfields
    call clear_allocated_fields

    if (nlev_in + 1 > nwzmax) then
      write(error_unit,'(A)') 'hybrid half-level count exceeds nwzmax'
      error stop 3
    end if
    do k = 1, nlev_in + 1
      akm(k) = ap_half(nlev_in + 2 - k)
      bkm(k) = b_half(nlev_in + 2 - k)
    end do
    akz(1) = 0.0
    bkz(1) = 1.0
    do k = 1, nlev_in
      akz(k+1) = 0.5 * (akm(k+1) + akm(k))
      bkz(k+1) = 0.5 * (bkm(k+1) + bkm(k))
    end do
    do k = 1, nlev_in + 1
      aknew(k) = akz(k)
      bknew(k) = bkz(k)
    end do
    nuvz = nlev_in + 1
    nwz = nlev_in + 1
    nz = nlev_in + 1
    ! ETA vertical bookkeeping used by find_z_level_eta / partoutput_eta.
    ! wheight ~ hybrid sigma-like coordinate (≈1 surface … ≈0 top).
    do k = 1, nwz
      wheight(k) = akm(k) / 101325.0 + bkm(k)
    end do
    uvheight(1) = 1.0
    do k = 2, nuvz
      uvheight(k) = akz(k) / 101325.0 + bkz(k)
    end do

    oro = surface_z_data(:,:,1) / ga
    excessoro = 0.0
    lsm = 0.0
    allocate(pv_raw(0:nx_in-1,0:ny_in-1,nlev_in))
    pv_raw = 0.0

    call load_window(1, 2)
    call alloc_interpol
    allocate(part(1))
  end subroutine initialise_flexpart_state

  subroutine load_window(left_index, right_index)
    integer, intent(in) :: left_index, right_index
    if (left_index == loaded_left .and. right_index == loaded_right) return
    if (left_index < 1 .or. right_index > nt_in .or. left_index >= right_index) then
      error stop 'invalid source window'
    end if
    call load_slot(1, left_index)
    call load_slot(2, right_index)
    loaded_left = left_index
    loaded_right = right_index
  end subroutine load_window

  subroutine load_slot(slot, source_time)
    integer, intent(in) :: slot, source_time
    integer :: ix, iy, kz, src
    real, allocatable :: uuh(:,:,:), vvh(:,:,:), wwh(:,:,:), pvh(:,:,:)
    ps(:,:,1,slot) = exp(lnsp_data(:,:,source_time))
    tt2(:,:,1,slot) = t2m_data(:,:,source_time)
    td2(:,:,1,slot) = d2m_data(:,:,source_time)
    u10(:,:,1,slot) = u10_data(:,:,source_time)
    v10(:,:,1,slot) = v10_data(:,:,source_time)
    allocate(uuh(0:nx_in-1,0:ny_in-1,nuvz), vvh(0:nx_in-1,0:ny_in-1,nuvz))
    allocate(wwh(0:nx_in-1,0:ny_in-1,nwz), pvh(0:nx_in-1,0:ny_in-1,nuvz))
    uuh = 0.0; vvh = 0.0; wwh = 0.0; pvh = 0.0
    do iy = 0, ny_in-1
      do ix = 0, nx_in-1
        uuh(ix,iy,1) = u10_data(ix+1,iy+1,source_time)
        vvh(ix,iy,1) = v10_data(ix+1,iy+1,source_time)
      end do
    end do
    do kz = 1, nlev_in
      src = nlev_in - kz + 1
      do iy = 0, ny_in-1
        do ix = 0, nx_in-1
          uuh(ix,iy,kz+1) = u_data(ix+1,iy+1,src,source_time)
          vvh(ix,iy,kz+1) = v_data(ix+1,iy+1,src,source_time)
          tth(ix,iy,kz+1,slot) = t_data(ix+1,iy+1,src,source_time)
          qvh(ix,iy,kz+1,slot) = q_data(ix+1,iy+1,src,source_time)
          wwh(ix,iy,kz+1) = omega_data(ix+1,iy+1,src,source_time)
        end do
      end do
    end do
    tth(:,:,1,slot) = tt2(:,:,1,slot)
    qvh(:,:,1,slot) = 0.0
    ! Real near-surface fluxes for formal calcpar path.
    sshf(:,:,1,slot) = ishf_data(:,:,source_time)
    if (metdata_format == GRIBFILE_CENTRE_NCEP) then
      hmix(:,:,1,slot) = blh_data(:,:,source_time)
    else
      hmix(:,:,1,slot) = 0.0
    end if
    call fill_sfcstress_from_zust(slot, source_time)
    call verttransform_ecmwf(slot, uuh, vvh, wwh, pvh)
    if (.not. have_pbl_inputs) then
      write(error_unit,'(A)') 'refusing calcpar: PBL inputs failed validation'
      error stop 7
    end if
    call oracle_calcpar(slot, uuh, vvh)
    deallocate(uuh, vvh, wwh, pvh)
  end subroutine load_slot

  subroutine clear_allocated_fields
    uu = 0.0; vv = 0.0; ww = 0.0
    uupol = 0.0; vvpol = 0.0
    tt = 0.0; tth = 0.0; pv = 0.0; qv = 0.0; qvh = 0.0
    rho = 0.0; drhodz = 0.0; pplev = 0.0; prs = 0.0; rho_dry = 0.0
    etauvheight = 0.0; etawheight = 0.0
    clwc = 0.0; ciwc = 0.0; clwch = 0.0; ciwch = 0.0
    ctwc = 0.0; icloudbot = 0; icloudtop = 0
    ps = 0.0; sd = 0.0; msl = 0.0; tcc = 0.0
    u10 = 0.0; v10 = 0.0; tt2 = 0.0; td2 = 0.0
    lsprec = 0.0; convprec = 0.0
    sshf = 0.0; ssr = 0.0; sfcstress = 0.0
    ustar = 0.0; wstar = 0.0; hmix = 0.0; tropopause = 0.0; oli = 0.0
    height = 0.0; wheight = 0.0; uvheight = 0.0
  end subroutine clear_allocated_fields

  subroutine run_queries
    integer :: q_unit, out_unit, ios, query_count, record_index
    integer :: kind, vertical_code, time_index, level_index, left_index, right_index
    integer(int64) :: time_relative
    real :: lon, lat, vertical_value
    character(len=point_id_len) :: point_id

    open(newunit=q_unit, file=trim(query_path), status='old', action='read', iostat=ios)
    if (ios /= 0) error stop 'cannot open query file'
    open(newunit=out_unit, file=trim(output_path), status='replace', action='write', iostat=ios)
    if (ios /= 0) error stop 'cannot open output file'
    read(q_unit,*,iostat=ios) query_count
    if (ios /= 0 .or. query_count < 1) error stop 'invalid query count'
    write(out_unit,'(A)') 'point_id'//achar(9)//'status'//achar(9)//'eastward_wind'// &
      achar(9)//'northward_wind'//achar(9)//'air_temperature'//achar(9)// &
      'specific_humidity'//achar(9)//'air_pressure'//achar(9)// &
      'geopotential_height'//achar(9)//'legacy_w'//achar(9)//'air_density'// &
      achar(9)//'geometric_terrain_height'//achar(9)//'geometric_vertical_velocity'

    do record_index = 1, query_count
      read(q_unit,*,iostat=ios) kind, time_relative, lon, lat, vertical_code, vertical_value, &
        time_index, level_index, left_index, right_index, point_id
      if (ios /= 0) error stop 'invalid query row'
      select case (kind)
      case (native_anchor)
        call emit_native(out_unit, trim(point_id), time_index, level_index, lon, lat)
      case (interpolated_common)
        call emit_interpolated(out_unit, trim(point_id), int(time_relative), lon, lat, &
          vertical_code, vertical_value, left_index, right_index, .false.)
      case (surface_layer)
        ! Surface-layer queries exercise the real FLEXPART PBL profile path
        ! (interpol_pbl + interpol_pbl_short) in addition to interpol_wind.
        call emit_interpolated(out_unit, trim(point_id), int(time_relative), lon, lat, &
          vertical_code, vertical_value, left_index, right_index, .true.)
      case (modern_difference)
        call emit_interpolated(out_unit, trim(point_id), int(time_relative), lon, lat, &
          vertical_code, vertical_value, left_index, right_index, .false.)
      case default
        error stop 'unsupported query kind'
      end select
    end do
    close(q_unit)
    close(out_unit)
  end subroutine run_queries

  subroutine emit_native(out_unit, point_id, time_index, level_index, lon, lat)
    integer, intent(in) :: out_unit, time_index, level_index
    character(len=*), intent(in) :: point_id
    real, intent(in) :: lon, lat
    integer :: ix0, iy0, fp_idx
    real :: out_u, out_v, out_t, out_q, out_p, out_h, sp_local

    call nearest_grid_node(lon, lat, ix0, iy0)
    ! level_index is 1-based from the model top (query contract).
    if (time_index < 1 .or. time_index > nt_in .or. level_index < 1 .or. level_index > nlev_in) then
      error stop 'native query index outside input'
    end if
    out_u = u_data(ix0+1,iy0+1,level_index,time_index)
    out_v = v_data(ix0+1,iy0+1,level_index,time_index)
    out_t = t_data(ix0+1,iy0+1,level_index,time_index)
    out_q = q_data(ix0+1,iy0+1,level_index,time_index)
    sp_local = exp(lnsp_data(ix0+1,iy0+1,time_index))
    out_p = 0.5*(ap_half(level_index)+ap_half(level_index+1)) + &
            0.5*(b_half(level_index)+b_half(level_index+1))*sp_local
    ! Full-level height from FLEXPART ETA hydrostatic etauvheight + oro.
    ! Never substitute surface_z for native full-level geopotential height.
    call ensure_native_heights(time_index)
    fp_idx = nlev_in - level_index + 2
    if (fp_idx < 2 .or. fp_idx > nuvz) error stop 'native fp level map out of range'
    out_h = flexpart_height_to_common_geopotential_height( &
      oro(ix0,iy0) + etauvheight(ix0,iy0,fp_idx,1))
    call write_ok(out_unit, point_id, out_u, out_v, out_t, out_q, out_p, out_h, &
      omega_data(ix0+1,iy0+1,level_index,time_index), -1.0, &
      flexpart_height_to_geometric(oro(ix0,iy0)), &
      omega_data(ix0+1,iy0+1,level_index,time_index))
  end subroutine emit_native

  subroutine emit_interpolated(out_unit, point_id, itime, lon, lat, vertical_code, &
      vertical_value, left_index, right_index, use_pbl_path)
    use, intrinsic :: iso_fortran_env, only: real64
    integer, intent(in) :: out_unit, itime, vertical_code, left_index, right_index
    character(len=*), intent(in) :: point_id
    real, intent(in) :: lon, lat, vertical_value
    logical, intent(in) :: use_pbl_path
    real :: xt, yt, terrain, terrain_geometric, target_geometric
    real :: z_agl, out_u, out_v, out_t, out_q, out_p, out_rho
    real :: direct_u, direct_v, direct_w, part_w, rho_pbl, rhograd_pbl
    real(real64) :: xt_dp, yt_dp, z_dp, zeta_dp
    real :: z_eta
    logical :: available

    if (left_index < 1 .or. right_index > nt_in .or. left_index >= right_index) then
      error stop 'invalid interpolation time bracket'
    end if
    call load_window(left_index, right_index)
    memtime(1) = int(valid_times(left_index) - valid_times(1))
    memtime(2) = int(valid_times(right_index) - valid_times(1))
    memind(1) = 1; memind(2) = 2; memind(3) = 2
    xt = (lon - xlon0) / dx
    yt = (lat - ylat0) / dy
    ! Bootstrap horizontal indices only; vertical ETA is set after z_to_zeta.
    call init_interpol(itime, xt, yt, 0.0, uvheight(1))
    call hor_interpol(oro, terrain)

    select case (vertical_code)
    case (vertical_agl)
      terrain_geometric = flexpart_height_to_geometric(terrain)
      target_geometric = terrain_geometric + vertical_value
      z_agl = geometric_to_flexpart_height(target_geometric) - terrain
      available = z_agl >= 0.0 .and. height(nz) > 0.0 .and. z_agl <= height(nz)
    case (vertical_asl)
      z_agl = geometric_to_flexpart_height(vertical_value) - terrain
      available = z_agl >= 0.0 .and. height(nz) > 0.0 .and. z_agl <= height(nz)
    case (vertical_pressure)
      call solve_pressure_height(itime, xt, yt, vertical_value, z_agl, available)
    case default
      error stop 'invalid vertical selector code'
    end select

    if (.not. available) then
      write(out_unit,'(A,A)') trim(point_id), achar(9)//'not_available'
      return
    end if

    ! Convert meter AGL → ETA coordinate (required by ETA interpol_wind/partoutput).
    xt_dp = real(xt, kind=real64)
    yt_dp = real(yt, kind=real64)
    z_dp = real(z_agl, kind=real64)
    zeta_dp = real(uvheight(1), kind=real64)
    call z_to_zeta(itime, xt_dp, yt_dp, z_dp, zeta_dp)
    z_eta = real(zeta_dp)
    if (.not. (z_eta == z_eta) .or. z_eta < 0.0 .or. z_eta > 1.5) then
      write(out_unit,'(A,A)') trim(point_id), achar(9)//'not_available'
      return
    end if

    call init_interpol(itime, xt, yt, z_agl, z_eta)

    if (use_pbl_path) then
      ! Real FLEXPART near-surface path used by advance_mod:
      ! interpol_pbl fills ust/wst/ol profiles; interpol_pbl_short returns density.
      call interpol_pbl(itime, xt, yt, z_agl, z_eta, 1)
      call interpol_pbl_short(z_agl, rho_pbl, rhograd_pbl, 1)
      if (.not. (rho_pbl > 0.0)) then
        write(error_unit,'(A)') 'interpol_pbl path produced non-physical surface density'
        error stop 6
      end if
    end if

    call interpol_wind(itime, xt, yt, z_agl, z_eta)
    direct_u = fp_u; direct_v = fp_v; direct_w = fp_w
    part(1)%z = z_agl
    part(1)%zeta = zeta_dp
    dz1out = -1.0
    out_u = 0.0; call interpol_partoutput_val('UU', out_u, 1)
    out_v = 0.0; call interpol_partoutput_val('VV', out_v, 1)
    out_t = 0.0; call interpol_partoutput_val('TT', out_t, 1)
    out_q = 0.0; call interpol_partoutput_val('QV', out_q, 1)
    out_p = 0.0; call interpol_partoutput_val('PR', out_p, 1)
    part_w = 0.0; call interpol_partoutput_val('WW', part_w, 1)
    out_rho = 0.0; call interpol_partoutput_val('RH', out_rho, 1)
    ! Under ETA, interpol_wind and partoutput can legitimately differ on W
    ! (eta-dot vs geometric legacy path). Keep UV within a loose bound and
    ! trust partoutput thermodynamic samples as the oracle values.
    if (abs(out_u-direct_u) > 1.0e-2 .or. abs(out_v-direct_v) > 1.0e-2) then
      write(error_unit,'(A,2ES16.8,1X,2ES16.8)') &
        "interpol_wind/partoutput UV disagree", out_u, direct_u, out_v, direct_v
      error stop 6
    end if
    if (use_pbl_path) then
      ! Prefer the density obtained through the PBL short path when finite.
      if (rho_pbl > 0.0) out_rho = rho_pbl
    end if
    if (.not. (out_u == out_u) .or. .not. (out_v == out_v) .or. &
        .not. (out_t == out_t) .or. .not. (out_q == out_q) .or. &
        .not. (out_p == out_p)) then
      write(out_unit,'(A,A)') trim(point_id), achar(9)//'not_available'
      return
    end if
    call write_ok(out_unit, point_id, out_u, out_v, out_t, out_q, out_p, &
      flexpart_height_to_common_geopotential_height(terrain + z_agl), direct_w, out_rho, &
      flexpart_height_to_geometric(terrain), direct_w)
  end subroutine emit_interpolated

  pure real function geometric_to_flexpart_height(geometric_height)
    real, intent(in) :: geometric_height
    real :: geopotential
    geopotential = common_g0 * common_earth_radius * geometric_height / &
      (common_earth_radius + geometric_height)
    geometric_to_flexpart_height = geopotential / ga
  end function geometric_to_flexpart_height

  pure real function flexpart_height_to_geometric(flexpart_height)
    real, intent(in) :: flexpart_height
    real :: geopotential
    geopotential = flexpart_height * ga
    flexpart_height_to_geometric = common_earth_radius * geopotential / &
      (common_g0 * common_earth_radius - geopotential)
  end function flexpart_height_to_geometric

  pure real function flexpart_height_to_common_geopotential_height(flexpart_height)
    real, intent(in) :: flexpart_height
    flexpart_height_to_common_geopotential_height = flexpart_height * ga / common_g0
  end function flexpart_height_to_common_geopotential_height

  subroutine solve_pressure_height(itime, xt, yt, target_pressure, z_result, available)
    integer, intent(in) :: itime
    real, intent(in) :: xt, yt, target_pressure
    real, intent(out) :: z_result
    logical, intent(out) :: available
    integer :: iteration
    real :: low, high, mid, p_low, p_high, p_mid

    low = 0.0; high = height(nz)
    call pressure_at_height(itime, xt, yt, low, p_low)
    call pressure_at_height(itime, xt, yt, high, p_high)
    if (target_pressure > p_low .or. target_pressure < p_high) then
      available = .false.; z_result = 0.0; return
    end if
    do iteration = 1, 36
      mid = 0.5 * (low + high)
      call pressure_at_height(itime, xt, yt, mid, p_mid)
      if (p_mid > target_pressure) then
        low = mid
      else
        high = mid
      end if
    end do
    z_result = 0.5 * (low + high)
    available = .true.
  end subroutine solve_pressure_height

  subroutine pressure_at_height(itime, xt, yt, z_agl, pressure)
    use, intrinsic :: iso_fortran_env, only: real64
    integer, intent(in) :: itime
    real, intent(in) :: xt, yt, z_agl
    real, intent(out) :: pressure
    real(real64) :: xt_dp, yt_dp, z_dp, zeta_dp
    real :: z_eta
    xt_dp = real(xt, kind=real64)
    yt_dp = real(yt, kind=real64)
    z_dp = real(z_agl, kind=real64)
    zeta_dp = real(uvheight(1), kind=real64)
    call z_to_zeta(itime, xt_dp, yt_dp, z_dp, zeta_dp)
    z_eta = real(zeta_dp)
    call interpol_wind(itime, xt, yt, z_agl, z_eta)
    part(1)%z = z_agl
    part(1)%zeta = zeta_dp
    dz1out = -1.0
    pressure = 0.0
    call interpol_partoutput_val('PR', pressure, 1)
  end subroutine pressure_at_height

  subroutine nearest_grid_node(lon, lat, ix0, iy0)
    real, intent(in) :: lon, lat
    integer, intent(out) :: ix0, iy0
    ix0 = nint((lon - xlon0) / dx)
    iy0 = nint((lat - ylat0) / dy)
    if (ix0 < 0 .or. ix0 >= nx_in .or. iy0 < 0 .or. iy0 >= ny_in) then
      error stop 'native query point outside grid'
    end if
  end subroutine nearest_grid_node

  subroutine write_ok(unit, point_id, out_u, out_v, out_t, out_q, out_p, out_h, &
      out_w, out_rho, out_terrain_geometric, out_geometric_w)
    integer, intent(in) :: unit
    character(len=*), intent(in) :: point_id
    real, intent(in) :: out_u, out_v, out_t, out_q, out_p, out_h, out_w, out_rho
    real, intent(in) :: out_terrain_geometric, out_geometric_w
    character(len=1), parameter :: tab = achar(9)
    write(unit,'(A,A,A,10(A,ES25.16E3))') trim(point_id), tab, 'ok', &
      tab, out_u, tab, out_v, tab, out_t, tab, out_q, tab, out_p, tab, out_h, &
      tab, out_w, tab, out_rho, tab, out_terrain_geometric, tab, out_geometric_w
  end subroutine write_ok

  subroutine read_real_1d(ncid, name, values)
    integer, intent(in) :: ncid
    character(len=*), intent(in) :: name
    real, intent(out) :: values(:)
    integer :: varid
    call check_nc(nf90_inq_varid(ncid, name, varid), 'variable '//name)
    call check_nc(nf90_get_var(ncid, varid, values), 'read '//name)
  end subroutine read_real_1d

  subroutine read_i64_1d(ncid, name, values)
    integer, intent(in) :: ncid
    character(len=*), intent(in) :: name
    integer(int64), intent(out) :: values(:)
    integer :: varid
    call check_nc(nf90_inq_varid(ncid, name, varid), 'variable '//name)
    call check_nc(nf90_get_var(ncid, varid, values), 'read '//name)
  end subroutine read_i64_1d

  subroutine read_real_3d(ncid, name, values)
    integer, intent(in) :: ncid
    character(len=*), intent(in) :: name
    real, intent(out) :: values(:,:,:)
    integer :: varid
    call check_nc(nf90_inq_varid(ncid, name, varid), 'variable '//name)
    call check_nc(nf90_get_var(ncid, varid, values), 'read '//name)
  end subroutine read_real_3d

  subroutine read_real_4d(ncid, name, values)
    integer, intent(in) :: ncid
    character(len=*), intent(in) :: name
    real, intent(out) :: values(:,:,:,:)
    integer :: varid
    call check_nc(nf90_inq_varid(ncid, name, varid), 'variable '//name)
    call check_nc(nf90_get_var(ncid, varid, values), 'read '//name)
  end subroutine read_real_4d

  subroutine reverse_latitude_3d(source, target)
    real, intent(in) :: source(:,:,:)
    real, intent(out) :: target(:,:,:)
    integer :: j
    do j = 1, ny_in
      target(:,j,:) = source(:,ny_in-j+1,:)
    end do
  end subroutine reverse_latitude_3d

  subroutine reverse_latitude_4d(source, target)
    real, intent(in) :: source(:,:,:,:)
    real, intent(out) :: target(:,:,:,:)
    integer :: j
    do j = 1, ny_in
      target(:,j,:,:) = source(:,ny_in-j+1,:,:)
    end do
  end subroutine reverse_latitude_4d

  subroutine assert_strictly_increasing(values, name)
    real, intent(in) :: values(:)
    character(len=*), intent(in) :: name
    if (any(values(2:) <= values(:size(values)-1))) then
      write(error_unit,'(A,A)') trim(name), ' is not strictly increasing'
      error stop 5
    end if
  end subroutine assert_strictly_increasing

  subroutine assert_strictly_decreasing(values, name)
    real, intent(in) :: values(:)
    character(len=*), intent(in) :: name
    if (any(values(2:) >= values(:size(values)-1))) then
      write(error_unit,'(A,A)') trim(name), ' is not strictly decreasing'
      error stop 5
    end if
  end subroutine assert_strictly_decreasing

  subroutine assert_strictly_increasing_i64(values, name)
    integer(int64), intent(in) :: values(:)
    character(len=*), intent(in) :: name
    if (any(values(2:) <= values(:size(values)-1))) then
      write(error_unit,'(A,A)') trim(name), ' is not strictly increasing'
      error stop 5
    end if
  end subroutine assert_strictly_increasing_i64

  subroutine fill_sfcstress_from_zust(slot, source_time)
    integer, intent(in) :: slot, source_time
    integer :: ix, iy
    real :: e, tv, rhoa, ust
    do iy = 0, ny_in-1
      do ix = 0, nx_in-1
        e = ew(td2(ix,iy,1,slot), ps(ix,iy,1,slot))
        tv = tt2(ix,iy,1,slot) * (1.0 + 0.378 * e / max(ps(ix,iy,1,slot), 1.0))
        rhoa = ps(ix,iy,1,slot) / (r_air * max(tv, 1.0))
        ust = zust_data(ix+1,iy+1,source_time)
        if (ust < 0.0) ust = 0.0
        sfcstress(ix,iy,1,slot) = (ust * ust) * max(rhoa, 1.0e-3)
      end do
    end do
  end subroutine fill_sfcstress_from_zust

  subroutine ensure_native_heights(time_index)
    integer, intent(in) :: time_index
    integer :: left_index, right_index
    if (time_index < nt_in) then
      left_index = time_index
      right_index = time_index + 1
    else
      left_index = max(1, nt_in - 1)
      right_index = nt_in
    end if
    call load_window(left_index, right_index)
    if (left_index /= time_index) then
      call load_slot(1, time_index)
      loaded_left = -1
      loaded_right = -1
    end if
  end subroutine ensure_native_heights

  subroutine try_read_real_3d(ncid, name, scratch, dest)
    integer, intent(in) :: ncid
    character(len=*), intent(in) :: name
    real, intent(inout) :: scratch(:,:,:)
    real, intent(out) :: dest(:,:,:)
    integer :: varid, status
    status = nf90_inq_varid(ncid, trim(name), varid)
    if (status /= nf90_noerr) then
      dest = -1.0
      if (name == 'ishf' .or. name == 'ie' .or. name == 'fsr') dest = 0.0
      return
    end if
    call check_nc(nf90_get_var(ncid, varid, scratch), 'read '//trim(name))
    call reverse_latitude_3d(scratch, dest)
  end subroutine try_read_real_3d

  subroutine try_read_pbl_field(ncid, name, scratch, dest, present_flag, missing_fill)
    integer, intent(in) :: ncid
    character(len=*), intent(in) :: name
    real, intent(inout) :: scratch(:,:,:)
    real, intent(out) :: dest(:,:,:)
    logical, intent(out) :: present_flag
    real, intent(in) :: missing_fill
    integer :: varid, status
    status = nf90_inq_varid(ncid, trim(name), varid)
    if (status /= nf90_noerr) then
      dest = missing_fill
      present_flag = .false.
      return
    end if
    call check_nc(nf90_get_var(ncid, varid, scratch), 'read '//trim(name))
    call reverse_latitude_3d(scratch, dest)
    if (size(dest,1) /= nx_in .or. size(dest,2) /= ny_in .or. size(dest,3) /= nt_in) then
      write(error_unit,'(A,A)') 'PBL field shape mismatch: ', trim(name)
      error stop 7
    end if
    if (.not. all_finite_3d(dest)) then
      write(error_unit,'(A,A)') 'PBL field has non-finite values: ', trim(name)
      error stop 7
    end if
    present_flag = .true.
  end subroutine try_read_pbl_field

  subroutine set_oracle_identity_from_env
    character(len=64) :: token
    integer :: length, status
    call get_environment_variable('TRAJECTA_ORACLE_SOURCE_FAMILY', token, length, status)
    if (status /= 0 .or. length <= 0) then
      write(error_unit,'(A)') 'TRAJECTA_ORACLE_SOURCE_FAMILY must be era5'
      error stop 8
    end if
    token = adjustl(token)
    if (.not. (token(1:4) == 'era5' .or. token(1:4) == 'ERA5')) then
      write(error_unit,'(A,A)') 'hybrid driver requires source_family=era5, got ', trim(token)
      error stop 8
    end if
    source_family = 'era5'

    call get_environment_variable('TRAJECTA_ORACLE_VERTICAL_COORDINATE', token, length, status)
    if (status /= 0 .or. length <= 0) then
      write(error_unit,'(A)') 'TRAJECTA_ORACLE_VERTICAL_COORDINATE must be hybrid_eta'
      error stop 8
    end if
    token = adjustl(token)
    if (.not. (token(1:10) == 'hybrid_eta' .or. token(1:10) == 'HYBRID_ETA')) then
      write(error_unit,'(A,A)') 'hybrid driver requires vertical_coordinate=hybrid_eta, got ', &
        trim(token)
      error stop 8
    end if
    vertical_coordinate = 'hybrid_eta'

    call get_environment_variable('TRAJECTA_ORACLE_PBL_HEIGHT_MODE', token, length, status)
    if (status /= 0 .or. length <= 0) then
      write(error_unit,'(A)') &
        'TRAJECTA_ORACLE_PBL_HEIGHT_MODE must be richardson_diagnosed'
      error stop 8
    end if
    token = adjustl(token)
    if (.not. (token(1:21) == 'richardson_diagnosed' .or. token(1:21) == 'RICHARDSON_DIAGNOSED')) then
      write(error_unit,'(A,A)') &
        'hybrid driver requires pbl_height_mode=richardson_diagnosed, got ', trim(token)
      error stop 8
    end if
    pbl_height_mode = 'richardson_diagnosed'
    call set_oracle_pbl_height_mode(PBL_HEIGHT_RICHARDSON_DIAGNOSED)

    ! Internal FLEXPART hybrid helper branch only. NOT a mixed identity token.
    metdata_format = GRIBFILE_CENTRE_ECMWF

    write(error_unit,'(A,A,A,A,A,A,A,I0)') &
      'oracle identity source_family=', trim(source_family), &
      ' vertical_coordinate=', trim(vertical_coordinate), &
      ' pbl_height_mode=', trim(pbl_height_mode), &
      ' pbl_mode_code=', oracle_pbl_height_mode
  end subroutine set_oracle_identity_from_env

  subroutine validate_pbl_inputs
    logical :: ok_ishf, ok_ust
    ok_ishf = pbl_has_ishf .and. all_finite_3d(ishf_data)
    ok_ust = pbl_has_ustar .and. all_finite_3d(zust_data) .and. all(zust_data >= 0.0)
    have_pbl_inputs = ok_ishf .and. ok_ust
    if (.not. have_pbl_inputs) then
      write(error_unit,'(A,2L2,1X,A,A)') &
        'PBL validation failed ishf/ustar=', ok_ishf, ok_ust, &
        'source_family=', trim(source_family)
      error stop 7
    end if
  end subroutine validate_pbl_inputs


  logical function all_finite_3d(arr)
    real, intent(in) :: arr(:,:,:)
    integer :: i, j, k
    all_finite_3d = .true.
    do k = 1, size(arr, 3)
      do j = 1, size(arr, 2)
        do i = 1, size(arr, 1)
          if (.not. ieee_is_finite(arr(i,j,k))) then
            all_finite_3d = .false.
            return
          end if
        end do
      end do
    end do
  end function all_finite_3d

  subroutine assert_finite_1d(arr, name)
    real, intent(in) :: arr(:)
    character(len=*), intent(in) :: name
    integer :: i
    do i = 1, size(arr)
      if (.not. ieee_is_finite(arr(i))) then
        write(error_unit,'(A,A)') 'non-finite values in ', trim(name)
        error stop 7
      end if
    end do
  end subroutine assert_finite_1d

  subroutine assert_finite_3d(arr, name)
    real, intent(in) :: arr(:,:,:)
    character(len=*), intent(in) :: name
    if (.not. all_finite_3d(arr)) then
      write(error_unit,'(A,A)') 'non-finite values in ', trim(name)
      error stop 7
    end if
  end subroutine assert_finite_3d

  subroutine assert_finite_4d(arr, name)
    real, intent(in) :: arr(:,:,:,:)
    character(len=*), intent(in) :: name
    integer :: i, j, k, n
    do n = 1, size(arr, 4)
      do k = 1, size(arr, 3)
        do j = 1, size(arr, 2)
          do i = 1, size(arr, 1)
            if (.not. ieee_is_finite(arr(i,j,k,n))) then
              write(error_unit,'(A,A)') 'non-finite values in ', trim(name)
              error stop 7
            end if
          end do
        end do
      end do
    end do
  end subroutine assert_finite_4d

  subroutine check_nc(status, context)
    integer, intent(in) :: status
    character(len=*), intent(in) :: context
    if (status /= nf90_noerr) then
      write(error_unit,'(A,A,A)') trim(context), ': ', trim(nf90_strerror(status))
      error stop 7
    end if
  end subroutine check_nc

end program eta_hybrid_oracle_driver
