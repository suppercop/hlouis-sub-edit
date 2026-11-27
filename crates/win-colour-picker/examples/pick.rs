//! Prints the colour under the cursor and a 9×9 magnifier, ten times a second for 5 s.
//!
//! `cargo run -p win-colour-picker --example pick`

use std::time::{Duration, Instant};
use win_colour_picker::magnifier_under_cursor;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(5) {
        let m = magnifier_under_cursor(4)?;
        let hex = m.center_colour().map_or_else(
            || "off-screen".to_owned(),
            |c| format!("#{:02X}{:02X}{:02X}", c.r, c.g, c.b),
        );
        println!(
            "cursor ({}, {}) dpi {:?}: {hex}",
            m.center.x, m.center.y, m.dpi
        );
        for dy in -4..=4 {
            let row: Vec<String> = (-4..=4)
                .map(|dx| {
                    m.at(dx, dy).map_or_else(
                        || "------".to_owned(),
                        |c| format!("{:02X}{:02X}{:02X}", c.r, c.g, c.b),
                    )
                })
                .collect();
            println!("  {}", row.join(" "));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}
