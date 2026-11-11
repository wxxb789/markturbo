#[test]
fn theme_tokens_parse() {
    nsh_base::tokens_of(Some(include_str!("../theme.json")))
        .expect("theme.json is a valid nsh theme");
}
