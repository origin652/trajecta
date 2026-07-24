! SPDX-FileCopyrightText: FLEXPART developers and Trajecta contributors
! SPDX-License-Identifier: GPL-3.0-or-later
!
! Standalone scalar extraction of the FLEXPART domain-fill ozone rule from
! par_mod.f90 and initdomain_mod.f90. Default REAL must remain 32-bit.
program ozone_pv60_scalar_driver
  implicit none

  real, parameter :: ozonescale = 60.0
  real, parameter :: pvcrit = 2.0
  real :: carrier_mass_kg, height_asl_m, latitude_degrees
  real :: potential_vorticity_pvu, hemisphere_pv, ozone_mass_kg
  integer :: eligible, status

  do
    read (*, *, iostat=status) carrier_mass_kg, height_asl_m, &
      latitude_degrees, potential_vorticity_pvu
    if (status < 0) exit
    if (status > 0) error stop "invalid ozone scalar input"

    hemisphere_pv = potential_vorticity_pvu
    if (latitude_degrees < 0.0) hemisphere_pv = -hemisphere_pv
    eligible = 0
    ozone_mass_kg = 0.0
    if ((height_asl_m > 3000.0) .and. (hemisphere_pv > pvcrit)) then
      eligible = 1
      ozone_mass_kg = carrier_mass_kg * hemisphere_pv * 48.0 / 29.0 * &
        ozonescale / 10.0**9
    end if

    write (*, '(I1,1X,ES24.16E3)') eligible, ozone_mass_kg
  end do
end program ozone_pv60_scalar_driver
