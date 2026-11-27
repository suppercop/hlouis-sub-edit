//! Live screen tests. They need an interactive desktop; without one they print why and skip.

use std::time::Instant;
use win_colour_picker::{
    cursor_position, magnifier_at, pick_under_cursor, pixel_at, virtual_screen, PickError, Point,
};

fn skip(e: &PickError) {
    eprintln!("SKIPPED win-colour-picker screen test: {e} (no interactive desktop?)");
}

#[test]
fn virtual_screen_and_cursor_are_physical_pixels() {
    let screen = virtual_screen();
    assert!(screen.width > 0 && screen.height > 0, "{screen:?}");
    match cursor_position() {
        Ok(p) => {
            // The cursor is always on some monitor; in physical pixels it lies inside the
            // physical virtual screen (a logical-pixel position would not on scaled displays
            // near the right / bottom edges, and this is what we check against).
            assert!(screen.contains(p), "{p:?} outside {screen:?}");
        }
        Err(e) => skip(&e),
    }
}

#[test]
fn nine_by_nine_magnifier_matches_single_pixel_reads() {
    let screen = virtual_screen();
    let center = Point {
        x: screen.x + screen.width / 2,
        y: screen.y + screen.height / 2,
    };
    let t = Instant::now();
    let m = match magnifier_at(center, 4) {
        Ok(m) => m,
        Err(e) => return skip(&e),
    };
    let elapsed = t.elapsed();
    assert_eq!(m.side(), 9);
    assert_eq!(m.pixels.len(), 81);
    assert!(
        m.pixels.iter().all(Option::is_some),
        "centre block is on screen"
    );
    if let Some(dpi) = m.dpi {
        assert!((96..=96 * 5).contains(&dpi), "dpi {dpi}");
    }
    let t = Instant::now();
    for _ in 0..10 {
        let _ = magnifier_at(center, 4);
    }
    let warm = t.elapsed() / 10;
    eprintln!(
        "win-colour-picker: 9x9 capture first {:.2} ms, then {:.2} ms avg, dpi {:?}, centre {:?}",
        elapsed.as_secs_f64() * 1e3,
        warm.as_secs_f64() * 1e3,
        m.dpi,
        m.center_colour()
    );

    // The screen may change between reads (clock, animations); retry a few times.
    let consistent = (0..5).any(|_| {
        let Ok(m) = magnifier_at(center, 4) else {
            return false;
        };
        [(0, 0), (-4, -4), (4, 4), (3, -2)].iter().all(|&(dx, dy)| {
            pixel_at(Point {
                x: center.x + dx,
                y: center.y + dy,
            })
            .ok()
            .flatten()
                == m.at(dx, dy)
        })
    });
    assert!(consistent, "magnifier and single-pixel reads disagree");
}

#[test]
fn block_past_the_screen_edge_is_masked() {
    let screen = virtual_screen();
    let corner = Point {
        x: screen.x,
        y: screen.y,
    };
    let m = match magnifier_at(corner, 4) {
        Ok(m) => m,
        Err(e) => return skip(&e),
    };
    // Left and top neighbours of the virtual-screen origin are off every monitor.
    assert_eq!(m.at(-1, 0), None);
    assert_eq!(m.at(0, -4), None);
    // At most the 5x5 quadrant right / below the origin can be on screen.
    assert!(m.pixels.iter().filter(|p| p.is_some()).count() <= 25);
}

#[test]
fn pick_under_cursor_returns_cursor_and_colour() {
    match pick_under_cursor() {
        Ok((p, colour)) => {
            assert!(virtual_screen().contains(p));
            eprintln!("win-colour-picker: cursor {p:?} colour {colour:?}");
        }
        Err(e) => skip(&e),
    }
}
