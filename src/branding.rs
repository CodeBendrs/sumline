// The public product name lives here. Storage and binary identifiers remain stable.
pub const NAME: &str = "SumLine";

pub fn index() -> String {
    let split = NAME
        .char_indices()
        .skip(1)
        .find(|(_, c)| c.is_uppercase())
        .map(|(index, _)| index)
        .unwrap_or(NAME.len());
    let (first, rest) = NAME.split_at(split);
    let escape = |s: &str| {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    };
    include_str!("../web/index.html")
        .replace("{{PRODUCT_NAME}}", &escape(NAME))
        .replace(
            "{{PRODUCT_WORDMARK}}",
            &format!("<b>{}</b>{}", escape(first), escape(rest)),
        )
}
