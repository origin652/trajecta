//! Line-oriented subject executable for the external GPL PV60 scalar oracle.

use std::error::Error;
use std::io::{self, BufRead};

use trajecta_core::population::{FlexpartPv60OzoneRule, OzoneAssignmentInput, OzoneAssignmentRule};

fn invalid_input(line_number: usize, message: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("line {line_number}: {message}"),
    )
}

fn main() -> Result<(), Box<dyn Error>> {
    let stdin = io::stdin();
    for (index, line) in stdin.lock().lines().enumerate() {
        let line_number = index + 1;
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let values = line
            .split_whitespace()
            .map(str::parse::<f64>)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| invalid_input(line_number, "expected four finite decimal values"))?;
        if values.len() != 4 || values.iter().any(|value| !value.is_finite()) {
            return Err(invalid_input(line_number, "expected four finite decimal values").into());
        }
        let assignment = FlexpartPv60OzoneRule
            .assign(OzoneAssignmentInput {
                carrier_dry_air_mass_kg: values[0],
                height_asl_m: values[1],
                latitude_degrees: values[2],
                potential_vorticity_pvu: values[3],
            })
            .map_err(|error| {
                invalid_input(
                    line_number,
                    &format!("ozone rule failed with {}", error.code()),
                )
            })?;
        let eligible = u8::from(assignment.ozone_mass_kg > 0.0);
        println!("{eligible} {:.17e}", assignment.ozone_mass_kg);
    }
    Ok(())
}
