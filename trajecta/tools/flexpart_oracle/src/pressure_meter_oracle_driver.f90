! SPDX-License-Identifier: GPL-3.0-or-later
!
! Minimal external FLEXPART oracle driver for pressure-level meteorology.
! This program is intentionally GPL-facing: it links the frozen FLEXPART
! modules and calls their real vertical transform and interpolation routines.

program pressure_meter_oracle_driver
  use, intrinsic :: iso_fortran_env, only: int64, error_unit
  use, intrinsic :: ieee_arithmetic, only: ieee_is_finite
  use netcdf
  use com_mod
  use windfields_mod
  use particle_mod, only: part
  use interpol_mod, only: alloc_interpol, init_interpol, hor_interpol, &
    interpol_wind, interpol_partoutput_val, interpol_pbl, interpol_pbl_short, &
    dz1out, fp_u => u, fp_v => v, fp_w => w
  use verttransform_mod, only: verttransform_gfs
  use oracle_calcpar_mod, only: oracle_calcpar, set_oracle_pbl_height_mode, &
    PBL_HEIGHT_OFFICIAL_PRESCRIBED, oracle_pbl_height_mode
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
  real, allocatable :: u_data(:,:,:,:), v_data(:,:,:,:), t_data(:,:,:,:)
  real, allocatable :: q_data(:,:,:,:), omega_data(:,:,:,:), z_data(:,:,:,:)
  real, allocatable :: sp_data(:,:,:), surface_z_data(:,:,:)
  real, allocatable :: t2m_data(:,:,:), d2m_data(:,:,:)
  real, allocatable :: u10_data(:,:,:), v10_data(:,:,:)
  real, allocatable :: blh_data(:,:,:), ishf_data(:,:,:), zust_data(:,:,:)
  real, allocatable :: fsr_data(:,:,:), ie_data(:,:,:)
  logical :: have_pbl_inputs = .false.
  logical :: pbl_has_ishf = .false., pbl_has_ustar = .false., pbl_has_blh = .false.
  ! Explicit oracle identity (not a single metdata_format conflation).
  character(len=16) :: source_family = ''
  character(len=16) :: vertical_coordinate = 'pressure'
  character(len=32) :: pbl_height_mode = 'official_prescribed'
  real, allocatable :: pv_raw(:,:,:)
  integer :: loaded_left = 0, loaded_right = 0

  call read_arguments
  call read_pressure_file
  call read_surface_file
  call initialise_flexpart_state
  call run_queries

contains

  subroutine read_arguments
    if (command_argument_count() /= 4) then
      write(error_unit,'(A)') 'usage: pressure_meter_oracle_driver PRESSURE.nc SURFACE.nc QUERIES.txt OUTPUT.tsv'
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
    call check_nc(nf90_inq_dimid(ncid, 'pressure_level', dimid), 'pressure dimension')
    call check_nc(nf90_inquire_dimension(ncid, dimid, len=nlev_in), 'pressure length')
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
    call read_real_1d(ncid, 'longitude', longitude)
    call read_real_1d(ncid, 'latitude', latitude_desc)
    call read_real_1d(ncid, 'pressure_level', pressure_hpa)
    call read_i64_1d(ncid, 'valid_time', valid_times)

    call assert_strictly_increasing(longitude, 'longitude')
    call assert_strictly_decreasing(latitude_desc, 'latitude')
    call assert_strictly_decreasing(pressure_hpa, 'pressure_level')
    call assert_strictly_increasing_i64(valid_times, 'valid_time')

    allocate(scratch4(nx_in,ny_in,nlev_in,nt_in))
    allocate(u_data(nx_in,ny_in,nlev_in,nt_in), v_data(nx_in,ny_in,nlev_in,nt_in))
    allocate(t_data(nx_in,ny_in,nlev_in,nt_in), q_data(nx_in,ny_in,nlev_in,nt_in))
    allocate(omega_data(nx_in,ny_in,nlev_in,nt_in), z_data(nx_in,ny_in,nlev_in,nt_in))

    call read_real_4d(ncid, 'u', scratch4); call reverse_latitude_4d(scratch4, u_data)
    call read_real_4d(ncid, 'v', scratch4); call reverse_latitude_4d(scratch4, v_data)
    call read_real_4d(ncid, 't', scratch4); call reverse_latitude_4d(scratch4, t_data)
    call read_real_4d(ncid, 'q', scratch4); call reverse_latitude_4d(scratch4, q_data)
    call read_real_4d(ncid, 'w', scratch4); call reverse_latitude_4d(scratch4, omega_data)
    call read_real_4d(ncid, 'z', scratch4); call reverse_latitude_4d(scratch4, z_data)
    deallocate(scratch4)
    call assert_finite_1d(longitude, 'longitude')
    call assert_finite_1d(latitude_desc, 'latitude')
    call assert_finite_1d(pressure_hpa, 'pressure_level')
    call assert_finite_4d(u_data, 'u')
    call assert_finite_4d(v_data, 'v')
    call assert_finite_4d(t_data, 't')
    call assert_finite_4d(q_data, 'q')
    call assert_finite_4d(omega_data, 'w')
    call assert_finite_4d(z_data, 'z')
    call check_nc(nf90_close(ncid), 'close pressure file')
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
    call read_real_3d(ncid, 'sp', scratch3); call reverse_latitude_3d(scratch3, sp_data)
    call read_real_3d(ncid, 'z', scratch3); call reverse_latitude_3d(scratch3, surface_z_data)
    call read_real_3d(ncid, 't2m', scratch3); call reverse_latitude_3d(scratch3, t2m_data)
    call read_real_3d(ncid, 'd2m', scratch3); call reverse_latitude_3d(scratch3, d2m_data)
    call read_real_3d(ncid, 'u10', scratch3); call reverse_latitude_3d(scratch3, u10_data)
    call read_real_3d(ncid, 'v10', scratch3); call reverse_latitude_3d(scratch3, v10_data)
    call try_read_pbl_field(ncid, 'blh', scratch3, blh_data, pbl_has_blh, -1.0)
    call try_read_pbl_field(ncid, 'ishf', scratch3, ishf_data, pbl_has_ishf, 0.0)
    call try_read_pbl_field(ncid, 'zust', scratch3, zust_data, pbl_has_ustar, -1.0)
    if (.not. pbl_has_ustar) then
      call try_read_pbl_field(ncid, 'fricv', scratch3, zust_data, pbl_has_ustar, -1.0)
    end if
    call try_read_real_3d(ncid, 'fsr', scratch3, fsr_data)
    call try_read_real_3d(ncid, 'ie', scratch3, ie_data)
    ! Explicit identity must be known before PBL requirement is applied.
    call set_oracle_identity_from_env
    call validate_pbl_inputs
    call assert_finite_3d(sp_data, 'sp')
    call assert_finite_3d(surface_z_data, 'surface z')
    call assert_finite_3d(t2m_data, 't2m')
    call assert_finite_3d(d2m_data, 'd2m')
    call assert_finite_3d(u10_data, 'u10')
    call assert_finite_3d(v10_data, 'v10')
    call assert_finite_3d(ishf_data, 'ishf')
    call assert_finite_3d(zust_data, 'zust/fricv')
    call assert_finite_3d(blh_data, 'blh')
    deallocate(scratch3, surface_times, surface_lon, surface_lat)
    call check_nc(nf90_close(ncid), 'close surface file')
  end subroutine read_surface_file

  subroutine initialise_flexpart_state
    integer :: k

    nx = nx_in; ny = ny_in; nxfield = nx_in
    nxmax = nx_in; nymax = ny_in
    nxmin1 = nx_in - 1; nymin1 = ny_in - 1
    nuvz = nlev_in; nwz = nlev_in; nz = nlev_in
    nuvzmax = nlev_in; nwzmax = nlev_in; nzmax = nlev_in
    nconvlevmax = nlev_in; na = nlev_in
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
    ! identity already fixed during surface PBL validation

    call alloc_fixedfields
    call alloc_windfields
    call clear_allocated_fields

    do k = 1, nlev_in
      akm(k) = pressure_hpa(k) * 100.0
      bkm(k) = 0.0
      akz(k) = akm(k)
      bkz(k) = 0.0
      aknew(k) = akm(k)
      bknew(k) = 0.0
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
    integer :: ix, iy
    real :: e, tv, rhoa, ust
    real, allocatable :: uuh(:,:,:), vvh(:,:,:)
    ps(:,:,1,slot) = sp_data(:,:,source_time)
    tt2(:,:,1,slot) = t2m_data(:,:,source_time)
    td2(:,:,1,slot) = d2m_data(:,:,source_time)
    u10(:,:,1,slot) = u10_data(:,:,source_time)
    v10(:,:,1,slot) = v10_data(:,:,source_time)
    tth(:,:,:,slot) = t_data(:,:,:,source_time)
    qvh(:,:,:,slot) = q_data(:,:,:,source_time)
    ! Real near-surface fluxes into FLEXPART calcpar path (not zero-filled).
    sshf(:,:,1,slot) = ishf_data(:,:,source_time)
    ! Pressure-coordinate path always uses official prescribed BLH/HPBL as hmix.
    hmix(:,:,1,slot) = blh_data(:,:,source_time)
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
    allocate(uuh(0:nx_in-1,0:ny_in-1,nuvz), vvh(0:nx_in-1,0:ny_in-1,nuvz))
    do iy = 0, ny_in-1
      do ix = 0, nx_in-1
        uuh(ix,iy,1:nuvz) = u_data(ix+1,iy+1,1:nuvz,source_time)
        vvh(ix,iy,1:nuvz) = v_data(ix+1,iy+1,1:nuvz,source_time)
      end do
    end do
    ! Fixed pressure levels: frozen FLEXPART GFS/pressure transform.
    call verttransform_gfs(slot, uuh, vvh, omega_data(:,:,:,source_time), pv_raw)
    ! Adapter bridge: GFS path stores pressure on the z-grid in pplev, while the
    ! meter-mode interpol_partoutput_val('PR') reads prs (ECMWF meter path).
    ! Mechanical field alias only — no formula change.
    prs(:,:,:,slot) = pplev(:,:,:,slot)
    if (.not. have_pbl_inputs) then
      write(error_unit,'(A)') 'refusing calcpar: PBL inputs failed validation'
      error stop 7
    end if
    call oracle_calcpar(slot, uuh, vvh)
    deallocate(uuh, vvh)
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
    integer :: ix0, iy0
    real :: out_u, out_v, out_t, out_q, out_p, out_h

    call nearest_grid_node(lon, lat, ix0, iy0)
    if (time_index < 1 .or. time_index > nt_in .or. level_index < 1 .or. level_index > nlev_in) then
      error stop 'native query index outside input'
    end if
    out_u = u_data(ix0+1,iy0+1,level_index,time_index)
    out_v = v_data(ix0+1,iy0+1,level_index,time_index)
    out_t = t_data(ix0+1,iy0+1,level_index,time_index)
    out_q = q_data(ix0+1,iy0+1,level_index,time_index)
    out_p = pressure_hpa(level_index) * 100.0
    ! The raw NetCDF field is geopotential.  Publish conventional
    ! geopotential height (Phi/g0), not FLEXPART's legacy Phi/ga height.
    out_h = z_data(ix0+1,iy0+1,level_index,time_index) / common_g0
    call write_ok(out_unit, point_id, out_u, out_v, out_t, out_q, out_p, out_h, &
      omega_data(ix0+1,iy0+1,level_index,time_index), -1.0, &
      flexpart_height_to_geometric(oro(ix0,iy0)), &
      omega_data(ix0+1,iy0+1,level_index,time_index))
  end subroutine emit_native

  subroutine emit_interpolated(out_unit, point_id, itime, lon, lat, vertical_code, &
      vertical_value, left_index, right_index, use_pbl_path)
    integer, intent(in) :: out_unit, itime, vertical_code, left_index, right_index
    character(len=*), intent(in) :: point_id
    real, intent(in) :: lon, lat, vertical_value
    logical, intent(in) :: use_pbl_path
    real :: xt, yt, terrain, terrain_geometric, target_geometric
    real :: z_agl, out_u, out_v, out_t, out_q, out_p, out_rho
    real :: direct_u, direct_v, direct_w, part_w, rho_pbl, rhograd_pbl
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
    call init_interpol(itime, xt, yt, 0.0, 0.0)
    call hor_interpol(oro, terrain)

    select case (vertical_code)
    case (vertical_agl)
      terrain_geometric = flexpart_height_to_geometric(terrain)
      target_geometric = terrain_geometric + vertical_value
      z_agl = geometric_to_flexpart_height(target_geometric) - terrain
      available = z_agl >= 0.0 .and. z_agl <= height(nz)
    case (vertical_asl)
      z_agl = geometric_to_flexpart_height(vertical_value) - terrain
      available = z_agl >= 0.0 .and. z_agl <= height(nz)
    case (vertical_pressure)
      call solve_pressure_height(itime, xt, yt, vertical_value, z_agl, available)
    case default
      error stop 'invalid vertical selector code'
    end select

    if (.not. available) then
      write(out_unit,'(A,A)') trim(point_id), achar(9)//'not_available'
      return
    end if

    if (use_pbl_path) then
      ! Real FLEXPART near-surface path used by advance_mod:
      ! interpol_pbl fills ust/wst/ol profiles; interpol_pbl_short returns density.
      call interpol_pbl(itime, xt, yt, z_agl, 0.0, 1)
      call interpol_pbl_short(z_agl, rho_pbl, rhograd_pbl, 1)
      if (.not. (rho_pbl > 0.0)) then
        write(error_unit,'(A)') 'interpol_pbl path produced non-physical surface density'
        error stop 6
      end if
    end if

    call interpol_wind(itime, xt, yt, z_agl, 0.0)
    direct_u = fp_u; direct_v = fp_v; direct_w = fp_w
    part(1)%z = z_agl
    dz1out = -1.0
    out_u = 0.0; call interpol_partoutput_val('UU', out_u, 1)
    out_v = 0.0; call interpol_partoutput_val('VV', out_v, 1)
    out_t = 0.0; call interpol_partoutput_val('TT', out_t, 1)
    out_q = 0.0; call interpol_partoutput_val('QV', out_q, 1)
    out_p = 0.0; call interpol_partoutput_val('PR', out_p, 1)
    part_w = 0.0; call interpol_partoutput_val('WW', part_w, 1)
    out_rho = 0.0; call interpol_partoutput_val('RH', out_rho, 1)
    if (abs(out_u-direct_u) > 1.0e-5 .or. abs(out_v-direct_v) > 1.0e-5 .or. &
        abs(part_w-direct_w) > 1.0e-5) then
      write(error_unit,'(A)') 'interpol_wind and interpol_partoutput_val disagree'
      error stop 6
    end if
    if (use_pbl_path) then
      ! Prefer the density obtained through the PBL short path when finite.
      if (rho_pbl > 0.0) out_rho = rho_pbl
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
    integer, intent(in) :: itime
    real, intent(in) :: xt, yt, z_agl
    real, intent(out) :: pressure
    call interpol_wind(itime, xt, yt, z_agl, 0.0)
    part(1)%z = z_agl
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
      write(error_unit,'(A)') 'TRAJECTA_ORACLE_SOURCE_FAMILY must be era5 or cfsr'
      error stop 8
    end if
    token = adjustl(token)
    if (token(1:4) == 'era5' .or. token(1:4) == 'ERA5') then
      source_family = 'era5'
    else if (token(1:4) == 'cfsr' .or. token(1:4) == 'CFSR') then
      source_family = 'cfsr'
    else
      write(error_unit,'(A,A)') 'unsupported TRAJECTA_ORACLE_SOURCE_FAMILY=', trim(token)
      error stop 8
    end if

    call get_environment_variable('TRAJECTA_ORACLE_VERTICAL_COORDINATE', token, length, status)
    if (status /= 0 .or. length <= 0) then
      write(error_unit,'(A)') 'TRAJECTA_ORACLE_VERTICAL_COORDINATE must be pressure'
      error stop 8
    end if
    token = adjustl(token)
    if (.not. (token(1:8) == 'pressure' .or. token(1:8) == 'PRESSURE')) then
      write(error_unit,'(A,A)') 'pressure driver requires vertical_coordinate=pressure, got ', &
        trim(token)
      error stop 8
    end if
    vertical_coordinate = 'pressure'

    call get_environment_variable('TRAJECTA_ORACLE_PBL_HEIGHT_MODE', token, length, status)
    if (status /= 0 .or. length <= 0) then
      write(error_unit,'(A)') 'TRAJECTA_ORACLE_PBL_HEIGHT_MODE must be official_prescribed'
      error stop 8
    end if
    token = adjustl(token)
    if (.not. (token(1:19) == 'official_prescribed' .or. token(1:19) == 'OFFICIAL_PRESCRIBED')) then
      write(error_unit,'(A,A)') &
        'pressure driver requires pbl_height_mode=official_prescribed, got ', trim(token)
      error stop 8
    end if
    pbl_height_mode = 'official_prescribed'
    call set_oracle_pbl_height_mode(PBL_HEIGHT_OFFICIAL_PRESCRIBED)

    ! Internal FLEXPART pressure-coordinate helper branch only. NOT source identity.
    metdata_format = GRIBFILE_CENTRE_NCEP

    write(error_unit,'(A,A,A,A,A,A,A,I0)') &
      'oracle identity source_family=', trim(source_family), &
      ' vertical_coordinate=', trim(vertical_coordinate), &
      ' pbl_height_mode=', trim(pbl_height_mode), &
      ' pbl_mode_code=', oracle_pbl_height_mode
  end subroutine set_oracle_identity_from_env

  subroutine validate_pbl_inputs
    logical :: ok_ishf, ok_ust, ok_blh
    ok_ishf = pbl_has_ishf .and. all_finite_3d(ishf_data)
    ok_ust = pbl_has_ustar .and. all_finite_3d(zust_data) .and. all(zust_data >= 0.0)
    ok_blh = pbl_has_blh .and. all_finite_3d(blh_data) .and. all(blh_data >= 0.0)
    have_pbl_inputs = ok_ishf .and. ok_ust .and. ok_blh
    if (.not. have_pbl_inputs) then
      write(error_unit,'(A,3L2,1X,A,A)') &
        'PBL validation failed ishf/ustar/blh=', ok_ishf, ok_ust, ok_blh, &
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

end program pressure_meter_oracle_driver
