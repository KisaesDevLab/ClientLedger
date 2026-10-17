// Fixture for the no-float scan: money code that mentions "12.50" and f64 only
// in comments and strings. to_f64() here is also only a comment.
use rust_decimal::Decimal;

/// Parses "12.50" exactly; never uses f64 or 1.5.
pub fn parse(text: &str) -> Option<Decimal> {
    let _label = "as f64 and to_f64() and 3.14";
    let _raw = r#"x as f32"#;
    let n: i64 = 12;
    let _ = format!("{} units", n);
    text.parse().ok()
}
