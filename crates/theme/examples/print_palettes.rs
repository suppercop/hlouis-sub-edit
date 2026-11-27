//! Prints an HTML swatch sheet of all themes and `EyeComfort` strengths for visual review.
//! Run with: `cargo run -p theme --example print_palettes > palettes.html`

use std::fmt::Write as _;
use theme::{contrast, palette, BaseTheme, ThemeSpec};

fn main() {
    let specs = [
        ("Light", ThemeSpec::Light),
        ("Dark", ThemeSpec::Dark),
        (
            "EyeComfort Light (30%)",
            ThemeSpec::EyeComfort {
                base: BaseTheme::Light,
                strength: 30,
            },
        ),
        (
            "EyeComfort Light (60%)",
            ThemeSpec::EyeComfort {
                base: BaseTheme::Light,
                strength: 60,
            },
        ),
        (
            "EyeComfort Light (100%)",
            ThemeSpec::EyeComfort {
                base: BaseTheme::Light,
                strength: 100,
            },
        ),
        (
            "EyeComfort Dark (30%)",
            ThemeSpec::EyeComfort {
                base: BaseTheme::Dark,
                strength: 30,
            },
        ),
        (
            "EyeComfort Dark (60%)",
            ThemeSpec::EyeComfort {
                base: BaseTheme::Dark,
                strength: 60,
            },
        ),
        (
            "EyeComfort Dark (100%)",
            ThemeSpec::EyeComfort {
                base: BaseTheme::Dark,
                strength: 100,
            },
        ),
    ];

    let mut html = String::from(
        "<!doctype html><html><head><meta charset='utf-8'><title>hlouis-sub-edit Palettes</title><style>body{font-family:system-ui,sans-serif;margin:24px;background:#1a1b1e;color:#e4e5e8}h2{margin-top:28px}.grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(190px,1fr));gap:8px}.sw{padding:10px;border-radius:6px;border:1px solid #444;font-size:12px}.box{height:28px;border-radius:4px;margin-bottom:6px;border:1px solid rgba(128,128,128,.35)}</style></head><body><h1>hlouis-sub-edit Theme Swatches</h1>",
    );

    for (label, spec) in specs {
        let p = palette(spec);
        let body_c = contrast(p.text_primary, p.window_bg);
        let _ = write!(
            html,
            "<h2>{label} — body contrast {body_c:.2}:1, mean linear blue {:.4}</h2><div class='grid'>",
            p.mean_linear_blue()
        );
        for (name, c) in p.all_tokens() {
            let hex = c.to_hex();
            let _ = write!(
                html,
                "<div class='sw'><div class='box' style='background:{hex}'></div><b>{name}</b><br>{hex}</div>"
            );
        }
        html.push_str("</div>");
    }
    html.push_str("</body></html>");
    println!("{html}");
}
