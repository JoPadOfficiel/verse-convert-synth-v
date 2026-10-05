//! Developer validator, never promotion into shipped pronunciation resources.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .ok_or("Correction JSON path required")?;
    let package = verse_lib::pronunciation::exchange::Package::read(std::path::Path::new(&path))?;
    println!("{}", serde_json::to_string(&package)?);
    Ok(())
}
