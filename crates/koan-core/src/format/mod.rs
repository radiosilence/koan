pub mod eval;
pub mod functions;
pub mod parser;

pub use eval::{MetadataProvider, evaluate};
pub use parser::{FormatError, Token, parse};

/// Parse and evaluate a format string in one call.
pub fn format(template: &str, provider: &dyn MetadataProvider) -> Result<String, FormatError> {
    let tokens = parse(template)?;
    Ok(evaluate(&tokens, provider))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn provider(data: &[(&str, &str)]) -> HashMap<String, String> {
        data.iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn basic_artist() {
        let p = provider(&[("album artist", "Radiohead")]);
        assert_eq!(format("%album artist%", &p).unwrap(), "Radiohead");
    }

    #[test]
    fn conditional_artist_album() {
        let p = provider(&[("album artist", "Radiohead"), ("album", "OK Computer")]);
        assert_eq!(
            format("[%album artist% - ]%album%", &p).unwrap(),
            "Radiohead - OK Computer"
        );
    }

    #[test]
    fn conditional_missing_artist() {
        let p = provider(&[("album", "OK Computer")]);
        assert_eq!(
            format("[%album artist% - ]%album%", &p).unwrap(),
            "OK Computer"
        );
    }

    #[test]
    fn quoted_literal_with_function() {
        let p = provider(&[("date", "1997-06-16"), ("album", "OK Computer")]);
        assert_eq!(
            format("['('$left(%date%,4)')' ]%album%", &p).unwrap(),
            "(1997) OK Computer"
        );
    }

    #[test]
    fn quoted_literal_missing_date() {
        let p = provider(&[("album", "OK Computer")]);
        assert_eq!(
            format("['('$left(%date%,4)')' ]%album%", &p).unwrap(),
            "OK Computer"
        );
    }

    #[test]
    fn complex_display() {
        let p = provider(&[
            ("tracknumber", "3"),
            ("title", "Subterranean Homesick Alien"),
            ("album artist", "Radiohead"),
            ("album", "OK Computer"),
            ("date", "1997-06-16"),
        ]);
        assert_eq!(
            format(
                "$num(%tracknumber%,2). %title% - [%album artist% - ]%album%[ '('$left(%date%,4)')']",
                &p
            )
            .unwrap(),
            "03. Subterranean Homesick Alien - Radiohead - OK Computer (1997)"
        );
    }

    #[test]
    fn if_function_integration() {
        let p = provider(&[("genre", "Rock")]);
        assert_eq!(format("$if(%genre%,%genre%,Unknown)", &p).unwrap(), "Rock");
    }

    #[test]
    fn if_function_missing() {
        let p = provider(&[]);
        assert_eq!(
            format("$if(%genre%,%genre%,Unknown)", &p).unwrap(),
            "Unknown"
        );
    }
}
