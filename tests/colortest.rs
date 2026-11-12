use terminalib::color;

#[test]
fn color_test() {
    let col = color::ColorPair {
        bg: color::Color::Red,
        fg: color::Color::Red,
    };

    let displayed = col.to_string();
    let converted = String::from(col);
    println!("{displayed}{converted}text{}", color::Color::reset());
    assert!(!displayed.is_empty());
    assert!(!converted.is_empty());
}
