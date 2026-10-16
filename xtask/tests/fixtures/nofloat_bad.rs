// Fixture for the no-float scan: five lines, five findings. Not compiled.
use rust_decimal::prelude::ToPrimitive;

fn bad(d: Decimal, n: serde_json::Number) {
    let a = d.to_f64();
    let b = Decimal::from_f64(1.0);
    let c = n.as_f64();
    let x = 1.5;
}
