! SPDX-License-Identifier: GPL-3.0-or-later
!
! Oracle-side PBL parameter bridge derived from FLEXPART getfields_mod::calcpar
! (commit dace3affa2ba71677f12f3858b04aaf59f8ee51e). Kept under the GPL oracle
! tree so drivers can call the same scalev/obukhov/richardson science without
! pulling drydepo/unc dependencies. Does not modify the frozen FLEXPART checkout.

module oracle_calcpar_mod
  use, intrinsic :: iso_fortran_env, only: error_unit
  use par_mod, only: r_air, ga, hmixmin, hmixmax
  use com_mod, only: lsubgrid
  use windfields_mod
  use qvsat_mod
  implicit none
  private
  public :: oracle_calcpar, oracle_richardson_fail_count
  public :: oracle_pbl_height_mode
  public :: PBL_HEIGHT_RICHARDSON_DIAGNOSED, PBL_HEIGHT_OFFICIAL_PRESCRIBED
  public :: set_oracle_pbl_height_mode
  ! Explicit PBL height mode (NOT source-family identity).
  ! Mirrors frozen FLEXPART calcpar branches without using metdata_format as identity.
  integer, parameter :: PBL_HEIGHT_RICHARDSON_DIAGNOSED = 1
  integer, parameter :: PBL_HEIGHT_OFFICIAL_PRESCRIBED = 2
  integer, save :: oracle_pbl_height_mode = 0
  ! Diagnostic only: must remain 0 for a scientifically complete oracle run.
  integer, save :: oracle_richardson_fail_count = 0

contains

subroutine set_oracle_pbl_height_mode(mode)
  integer, intent(in) :: mode
  if (mode /= PBL_HEIGHT_RICHARDSON_DIAGNOSED .and. &
      mode /= PBL_HEIGHT_OFFICIAL_PRESCRIBED) then
    write(error_unit,'(A,I0)') 'oracle_calcpar: invalid pbl_height_mode ', mode
    error stop 8
  end if
  oracle_pbl_height_mode = mode
end subroutine set_oracle_pbl_height_mode

subroutine oracle_calcpar(n, uuh_in, vvh_in)
  ! Compute ustar/oli/hmix/wstar into windfields arrays for mem index n.
  ! Requires:
  !   - sshf and sfcstress already loaded from real surface inputs
  !   - oracle_pbl_height_mode set by caller
  !   - official_prescribed: hmix pre-filled from official BLH/HPBL
  ! Mirrors frozen FLEXPART calcpar: Richardson failure is fatal (no silent fallback).
  ! pressure-coordinate prescribed branch is the frozen NCEP calcpar science path
  ! mechanically selected by pbl_height_mode (formulas/constants unchanged).
  integer, intent(in) :: n
  real, intent(in) :: uuh_in(0:,0:,:)
  real, intent(in) :: vvh_in(0:,0:,:)
  integer :: ix, jy, i, llev, ierr
  real :: ol, hmixplus, hmixdummy, akzdummy, subsceff
  real :: ttlev(nuvzmax), qvlev(nuvzmax), ulev(nuvzmax), vlev(nuvzmax)
  logical :: prescribed

  if (oracle_pbl_height_mode /= PBL_HEIGHT_RICHARDSON_DIAGNOSED .and. &
      oracle_pbl_height_mode /= PBL_HEIGHT_OFFICIAL_PRESCRIBED) then
    write(error_unit,'(A,I0)') &
      'oracle_calcpar: pbl_height_mode must be set by caller, got ', &
      oracle_pbl_height_mode
    error stop 8
  end if
  prescribed = oracle_pbl_height_mode == PBL_HEIGHT_OFFICIAL_PRESCRIBED

  do jy = 0, nymin1
    do ix = 0, nxmin1
      ustar(ix,jy,1,n) = scalev(ps(ix,jy,1,n), tt2(ix,jy,1,n), &
        td2(ix,jy,1,n), sfcstress(ix,jy,1,n))
      if (ustar(ix,jy,1,n) <= 1.e-8) ustar(ix,jy,1,n) = 1.e-8

      if (prescribed) then
        ! Frozen pressure-coordinate branch: first model level above local surface
        ! pressure, then Obukhov from that level (identical to NCEP calcpar loop).
        llev = 0
        do i = 1, nuvz
          if (ps(ix,jy,1,n) < akz(i)) llev = i
        end do
        llev = llev + 1
        if (llev > nuvz) llev = nuvz - 1
        ol = obukhov(ps(ix,jy,1,n), tt2(ix,jy,1,n), td2(ix,jy,1,n), &
          tth(ix,jy,llev,n), ustar(ix,jy,1,n), sshf(ix,jy,1,n), &
          akm, bkm, akz(llev), prescribed)
      else
        ol = obukhov(ps(ix,jy,1,n), tt2(ix,jy,1,n), td2(ix,jy,1,n), &
          tth(ix,jy,2,n), ustar(ix,jy,1,n), sshf(ix,jy,1,n), &
          akm, bkm, akzdummy, prescribed)
      end if
      if (ol /= 0.) then
        oli(ix,jy,1,n) = 1./ol
      else
        oli(ix,jy,1,n) = 99999.
      end if

      do i = 1, nuvz
        ulev(i) = uuh_in(ix,jy,i)
        vlev(i) = vvh_in(ix,jy,i)
        ttlev(i) = tth(ix,jy,i,n)
        qvlev(i) = qvh(ix,jy,i,n)
      end do

      if (prescribed) then
        ! Official BLH/HPBL already in hmix; richardson only supplies wstar/hmixplus.
        call richardson(ps(ix,jy,1,n), ustar(ix,jy,1,n), ttlev, qvlev, &
          ulev, vlev, nuvz, akz, bkz, sshf(ix,jy,1,n), tt2(ix,jy,1,n), &
          td2(ix,jy,1,n), hmixdummy, wstar(ix,jy,1,n), hmixplus, ierr)
      else
        call richardson(ps(ix,jy,1,n), ustar(ix,jy,1,n), ttlev, qvlev, &
          ulev, vlev, nuvz, akz, bkz, sshf(ix,jy,1,n), tt2(ix,jy,1,n), &
          td2(ix,jy,1,n), hmix(ix,jy,1,n), wstar(ix,jy,1,n), hmixplus, ierr)
      end if
      if (ierr < 0) then
        oracle_richardson_fail_count = oracle_richardson_fail_count + 1
        write(error_unit,'(A,3I6,A,I0)') &
          'oracle_calcpar: richardson failed at ix,jy,n=', ix, jy, n, &
          ' ierr=', ierr
        error stop 'oracle_calcpar: richardson failed (no silent scientific fallback)'
      end if

      if (lsubgrid == 1) then
        subsceff = min(excessoro(ix,jy), hmixplus)
      else
        subsceff = 0.0
      end if
      hmix(ix,jy,1,n) = hmix(ix,jy,1,n) + subsceff
      hmix(ix,jy,1,n) = max(hmixmin, hmix(ix,jy,1,n))
      hmix(ix,jy,1,n) = min(hmixmax, hmix(ix,jy,1,n))
    end do
  end do
end subroutine oracle_calcpar

real function obukhov(ps,tsfc,tdsfc,tlev,ustar,hf,akm,bkm,plev,prescribed_pbl)

  !********************************************************************
  !                                                                   *
  !                       Author: G. WOTAWA                           *
  !                       Date:   1994-06-27                          *
  !                                                                   *
  !     This program calculates Obukhov scale height from surface     *
  !     meteorological data and sensible heat flux.                   *
  !                                                                   *
  !********************************************************************
  !                                                                   *
  !  Update: A. Stohl, 2000-09-25, avoid division by zero by          *
  !  setting ustar to minimum value                                   *
  !  CHANGE: 17/11/2005 Caroline Forster NCEP GFS version             *
  !                                                                   *
  !   Unified ECMWF and GFS builds                                    *
  !   Marian Harustak, 12.5.2017                                      *
  !     - Merged obukhov and obukhov_gfs into one routine using       *
  !       if-then for meteo-type dependent code                       *
  !                                                                   *
  !  Oracle adapter: prescribed_pbl selects the frozen pressure-level *
  !  (NCEP) branch vs hybrid (ECMWF) branch without using             *
  !  metdata_format as source-family identity. Formulas unchanged.    *
  !                                                                   *
  !********************************************************************

  use qvsat_mod

  implicit none

  real,dimension(:) :: akm,bkm
  real :: ps,tsfc,tdsfc,tlev,ustar,hf,e,tv,rhoa,plev
  real :: ak1,bk1,theta,thetastar
  logical, intent(in) :: prescribed_pbl


  e=ew(tdsfc,ps)                           ! vapor pressure
  tv=tsfc*(1.+0.378*e/ps)               ! virtual temperature
  rhoa=ps/(r_air*tv)                      ! air density
  if (.not. prescribed_pbl) then
  ak1=(akm(1)+akm(2))*0.5
  bk1=(bkm(1)+bkm(2))*0.5
  plev=ak1+bk1*ps                        ! Pressure level 1
  end if
  theta=tlev*(100000./plev)**(r_air/cpa) ! potential temperature
  if (ustar.le.0.) ustar=1.e-8
  thetastar=hf/(rhoa*cpa*ustar)           ! scale temperature
  if(abs(thetastar).gt.1.e-10) then
     obukhov=theta*ustar**2/(karman*ga*thetastar)
  else
     obukhov=9999                        ! zero heat flux
  endif
  if (obukhov.gt. 9999.) obukhov= 9999.
  if (obukhov.lt.-9999.) obukhov=-9999.
end function obukhov

subroutine richardson(psfc,ust,ttlev,qvlev,ulev,vlev,nuvz, &
       akz,bkz,hf,tt2,td2,h,wst,hmixplus,ierr)
  !                        i    i    i     i    i    i    i
  ! i   i  i   i   i  o  o     o
  !****************************************************************************
  !                                                                           *
  !     Calculation of mixing height based on the critical Richardson number. *
  !     Calculation of convective time scale.                                 *
  !     For unstable conditions, one iteration is performed. An excess        *
  !     temperature (dependent on hf and wst) is calculated, added to the     *
  !     temperature at the lowest model level. Then the procedure is repeated.*
  !                                                                           *
  !     Author: A. Stohl                                                      *
  !                                                                           *
  !     22 August 1996                                                        *
  !                                                                           *
  !     Literature:                                                           *
  !     Vogelezang DHP and Holtslag AAM (1996): Evaluation and model impacts  *
  !     of alternative boundary-layer height formulations. Boundary-Layer     *
  !     Meteor. 81, 245-269.                                                  *
  !                                                                           *
  !****************************************************************************
  !                                                                           *
  !     Update: 1999-02-01 by G. Wotawa                                       *
  !                                                                           *
  !     Two meter level (temperature, humidity) is taken as reference level   *
  !     instead of first model level.                                         *
  !     New input variables tt2, td2 introduced.                              *
  !                                                                           *
  !     CHANGE: 17/11/2005 Caroline Forster NCEP GFS version                  *
  !                                                                           *
  !     Unified ECMWF and GFS builds                                          *
  !     Marian Harustak, 12.5.2017                                            *
  !       - Merged richardson and richardson_gfs into one routine using       *
  !         if-then for meteo-type dependent code                             *
  !                                                                           *
  !****************************************************************************
  !                                                                           *
  ! Variables:                                                                *
  ! h                          mixing height [m]                              *
  ! hf                         sensible heat flux                             *
  ! psfc                      surface pressure at point (xt,yt) [Pa]         *
  ! tv                         virtual temperature                            *
  ! wst                        convective velocity scale                      *
  ! metdata_format             format of metdata (ecmwf/gfs)                  *
  !                                                                           *
  ! Constants:                                                                *
  ! ric                        critical Richardson number                     *
  !                                                                           *
  !****************************************************************************

  use class_gribfile_mod
  use qvsat_mod

  implicit none

  integer,intent(out) ::            &
    ierr                              ! Returns error when no richardson number can be found
  real, intent(out) ::              &
    h,                              & ! mixing height [m]
    wst,                            & ! convective velocity scale
    hmixplus                          !
  integer,intent(in)  ::            &
    nuvz                              ! Upper vertical level
  real,intent(in) ::                &
    psfc,                           & ! surface pressure at point (xt,yt) [Pa]
    ust,                            & ! Scale velocity
    hf,                             & ! Surface sensible heat flux
    tt2,td2                           ! Temperature
  real,intent(in),dimension(:) ::   &
    ttlev,                          &
    qvlev,                          &
    ulev,                           &
    vlev,                           &
    akz,bkz
  integer ::                        &
    i,k,iter,llev,loop_start,kcheck   ! Loop variables
  real ::                           &
    tv,tvold,                       & ! Virtual temperature
    zref,z,zold,zl,zl1,zl2,         & ! Heights
    pint,pold,                      & ! Pressures
    theta,thetaold,thetaref,thetal, & ! Potential temperature
    theta1,theta2,thetam,           &
    ri,                             & ! Richardson number per level
    ril,                            & ! Richardson number sub level
    excess,                         & !
    ul,vl,                          & ! Velocities sub level
    wspeed,                         & ! Wind speed at z=hmix
    bvfsq,                          & ! Brunt-Vaisala frequency
    bvf,                            & ! square root of bvfsq
    rh,rhold,rhl
  real,parameter    :: const=r_air/ga, ric=0.25, b=100., bs=8.5
  integer,parameter :: itmax=3

  excess=0.0

  if (metdata_format.eq.GRIBFILE_CENTRE_NCEP) then
    ! NCEP version: find first model level above ground
    !**************************************************

     llev = 0
     do i=1,nuvz
       if (psfc.lt.akz(i)) llev=i
     end do
     llev = llev+1
    ! sec llev should not be 1!
     if (llev.eq.1) llev = 2
     if (llev.gt.nuvz) llev = nuvz-1
    ! NCEP version
  end if


  ! Compute virtual temperature and virtual potential temperature at
  ! reference level (2 m)
  !*****************************************************************

  do iter=1,itmax,1

    pold=psfc
    tvold=tt2*(1.+0.378*ew(td2,psfc)/psfc)
    zold=2.0
    zref=zold
    rhold=ew(td2,psfc)/ew(tt2,psfc)


    thetaref=tvold*(100000./pold)**(r_air/cpa)+excess
    thetaold=thetaref


    ! Integrate z up to one level above zt
    !*************************************
    if (metdata_format.eq.GRIBFILE_CENTRE_NCEP) then
      loop_start=llev
    else
      loop_start=2
    end if
    kcheck=loop_start
    do k=loop_start,nuvz
      kcheck=k
      pint=akz(k)+bkz(k)*psfc  ! pressure on model layers
      tv=ttlev(k)*(1.+0.608*qvlev(k))

      if (abs(tv-tvold).gt.0.2) then
        z=zold+const*log(pold/pint)*(tv-tvold)/log(tv/tvold)
      else
        z=zold+const*log(pold/pint)*tv
      endif

      theta=tv*(100000./pint)**(r_air/cpa)
    ! PS
      rh = qvlev(k) / f_qvsat( pint, ttlev(k) )


    ! Calculate Richardson number at each level
    !****************************************

      ri=ga/thetaref*(theta-thetaref)*(z-zref)/ &
           max(((ulev(k)-ulev(2))**2+(vlev(k)-vlev(2))**2+b*ust**2),0.1)

    !  addition of second condition: MH should not be placed in an
    !  unstable layer (PS / Feb 2000)
      if (ri.gt.ric .and. thetaold.lt.theta) exit

      tvold=tv
      pold=pint
      rhold=rh
      thetaold=theta
      zold=z
    end do
    ! Check opied from FLEXPART-WRF, 2022 LB
    if (kcheck.ge.nuvz) then
      write(*,*) 'richardson not working, no stable layer -- k = nuvz'
      ierr = -10
      goto 7000
    endif
    !k=min(k,nuvz) ! ESO: make sure k <= nuvz (ticket #139) !MD change to work without goto

    ! Determine Richardson number between the critical levels
    !********************************************************

    zl1=zold
    theta1=thetaold
    do i=1,20
      zl=zold+real(i)/20.*(z-zold)
      ul=ulev(kcheck-1)+real(i)/20.*(ulev(kcheck)-ulev(kcheck-1))
      vl=vlev(kcheck-1)+real(i)/20.*(vlev(kcheck)-vlev(kcheck-1))
      thetal=thetaold+real(i)/20.*(theta-thetaold)
      rhl=rhold+real(i)/20.*(rh-rhold)
      ril=ga/thetaref*(thetal-thetaref)*(zl-zref)/ &
           max(((ul-ulev(2))**2+(vl-vlev(2))**2+b*ust**2),0.1)
      zl2=zl
      theta2=thetal
      if (ril.gt.ric) exit
      if (i.eq.20) then
        write(*,*) 'WARNING: NO RICHARDSON NUMBER GREATER THAN 0.25 FOUND', kcheck,nuvz,ril,ri
        exit
      endif
      zl1=zl
      theta1=thetal
      if (i.eq.20) error stop 'RICHARDSON: NO RICHARDSON NUMBER GREATER THAN 0.25 FOUND'
    end do

    h=zl
    thetam=0.5*(theta1+theta2)
    wspeed=sqrt(ul**2+vl**2)                    ! Wind speed at z=hmix
    bvfsq=(ga/thetam)*(theta2-theta1)/(zl2-zl1) ! Brunt-Vaisala frequency
                                                ! at z=hmix

    ! Under stable conditions, limit the maximum effect of the subgrid-scale topography
    ! by the maximum lifting possible from the available kinetic energy
    !*****************************************************************************

    if(bvfsq.le.0.) then
      hmixplus=9999.
    else
      bvf=sqrt(bvfsq)
      hmixplus=wspeed/bvf*convke
    endif


    ! Calculate convective velocity scale
    !************************************

    if (hf.lt.0.) then
      wst=(-h*ga/thetaref*hf/cpa)**0.333
      excess=-bs*hf/cpa/wst
    else
      wst=0.
      exit
    endif
  end do

  ierr = 0
  return

! Fatal error -- print the inputs
7000  continue
  write(*,'(a         )') 'nuvz'
  write(*,'(i5        )')  nuvz
  write(*,'(a         )') 'psfc,ust,hf,tt2,td2,h,wst,hmixplus'
  write(*,'(1p,4e18.10)')  psfc,ust,hf,tt2,td2,h,wst,hmixplus
  return
end subroutine richardson

real function scalev(ps,t,td,stress)

  !********************************************************************
  !                                                                   *
  !                       Author: G. WOTAWA                           *
  !                       Date:   1994-06-27                          *
  !                       Update: 1996-05-21 A. Stohl                 *
  !                                                                   *
  !********************************************************************
  !                                                                   *
  !     This Programm calculates scale velocity ustar from surface    *
  !     stress and air density.                                       *
  !                                                                   *
  !********************************************************************
  !                                                                   *
  !     INPUT:                                                        *
  !                                                                   *
  !     ps      surface pressure [Pa]                                 *
  !     t       surface temperature [K]                               *
  !     td      surface dew point [K]                                 *
  !     stress  surface stress [N/m2]                                 *
  !                                                                   *
  !********************************************************************
  use qvsat_mod

  implicit none

  real :: ps,t,td,e,tv,rhoa,stress

  e=ew(td,ps)                       ! vapor pressure
  tv=t*(1.+0.378*e/ps)           ! virtual temperature
  rhoa=ps/(r_air*tv)              ! air density
  scalev=sqrt(abs(stress)/rhoa)
end function scalev

end module oracle_calcpar_mod
