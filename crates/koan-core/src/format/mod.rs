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

    // ---- Basic field/conditional/function tests ----

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

    // ======================================================================
    // Real-world pattern tests — the two patterns that MUST work perfectly
    // ======================================================================

    // Pattern 1: %album artist%/$if($stricmp(%album artist%,Various Artists),,['('$left(%date%,4)')' ])%album% '['%codec%']'/[$num(%discnumber%,2)][%tracknumber%. ][%artist% - ]%title%
    //
    // Logic: for VA albums, skip the date prefix. For normal albums, show (year) if date exists.

    const PATTERN_1: &str = "%album artist%/$if($stricmp(%album artist%,Various Artists),,['('$left(%date%,4)')' ])%album% '['%codec%']'/[$num(%discnumber%,2)][%tracknumber%. ][%artist% - ]%title%";

    #[test]
    fn pattern1_normal_album_full_metadata() {
        let p = provider(&[
            ("album artist", "Aphex Twin"),
            ("album", "Selected Ambient Works 85-92"),
            ("date", "1992-11-09"),
            ("codec", "FLAC"),
            ("discnumber", "1"),
            ("tracknumber", "01"),
            ("artist", "Aphex Twin"),
            ("title", "Xtal"),
        ]);
        assert_eq!(
            format(PATTERN_1, &p).unwrap(),
            "Aphex Twin/(1992) Selected Ambient Works 85-92 [FLAC]/0101. Aphex Twin - Xtal"
        );
    }

    #[test]
    fn pattern1_normal_album_no_disc() {
        let p = provider(&[
            ("album artist", "Aphex Twin"),
            ("album", "Selected Ambient Works 85-92"),
            ("date", "1992-11-09"),
            ("codec", "FLAC"),
            ("tracknumber", "01"),
            ("artist", "Aphex Twin"),
            ("title", "Xtal"),
        ]);
        assert_eq!(
            format(PATTERN_1, &p).unwrap(),
            "Aphex Twin/(1992) Selected Ambient Works 85-92 [FLAC]/01. Aphex Twin - Xtal"
        );
    }

    #[test]
    fn pattern1_normal_album_no_date() {
        let p = provider(&[
            ("album artist", "Aphex Twin"),
            ("album", "Selected Ambient Works 85-92"),
            ("codec", "FLAC"),
            ("tracknumber", "01"),
            ("artist", "Aphex Twin"),
            ("title", "Xtal"),
        ]);
        // No date → the conditional ['(' ... ')' ] is skipped entirely
        assert_eq!(
            format(PATTERN_1, &p).unwrap(),
            "Aphex Twin/Selected Ambient Works 85-92 [FLAC]/01. Aphex Twin - Xtal"
        );
    }

    #[test]
    fn pattern1_va_album_skips_date() {
        let p = provider(&[
            ("album artist", "Various Artists"),
            ("album", "Warp 20 Recreated"),
            ("date", "2009"),
            ("codec", "FLAC"),
            ("tracknumber", "03"),
            ("artist", "Flying Lotus"),
            ("title", "Roygbiv"),
        ]);
        // VA album → $stricmp matches → $if takes truthy (empty) branch → no date prefix
        assert_eq!(
            format(PATTERN_1, &p).unwrap(),
            "Various Artists/Warp 20 Recreated [FLAC]/03. Flying Lotus - Roygbiv"
        );
    }

    #[test]
    fn pattern1_va_case_insensitive() {
        let p = provider(&[
            ("album artist", "various artists"),
            ("album", "Compilation"),
            ("date", "2020"),
            ("codec", "MP3"),
            ("tracknumber", "01"),
            ("artist", "Some Artist"),
            ("title", "Some Track"),
        ]);
        assert_eq!(
            format(PATTERN_1, &p).unwrap(),
            "various artists/Compilation [MP3]/01. Some Artist - Some Track"
        );
    }

    #[test]
    fn pattern1_artist_same_as_album_artist() {
        let p = provider(&[
            ("album artist", "Radiohead"),
            ("album", "OK Computer"),
            ("date", "1997-06-16"),
            ("codec", "FLAC"),
            ("tracknumber", "01"),
            ("artist", "Radiohead"),
            ("title", "Airbag"),
        ]);
        // artist == album artist → still shows "Radiohead - " per the pattern
        assert_eq!(
            format(PATTERN_1, &p).unwrap(),
            "Radiohead/(1997) OK Computer [FLAC]/01. Radiohead - Airbag"
        );
    }

    #[test]
    fn pattern1_no_artist_tag() {
        let p = provider(&[
            ("album artist", "Radiohead"),
            ("album", "OK Computer"),
            ("date", "1997"),
            ("codec", "FLAC"),
            ("tracknumber", "01"),
            ("title", "Airbag"),
        ]);
        // No artist tag → the [%artist% - ] conditional is skipped
        assert_eq!(
            format(PATTERN_1, &p).unwrap(),
            "Radiohead/(1997) OK Computer [FLAC]/01. Airbag"
        );
    }

    #[test]
    fn pattern1_multi_disc() {
        let p = provider(&[
            ("album artist", "Tool"),
            ("album", "10,000 Days"),
            ("date", "2006"),
            ("codec", "FLAC"),
            ("discnumber", "1"),
            ("tracknumber", "01"),
            ("artist", "Tool"),
            ("title", "Vicarious"),
        ]);
        assert_eq!(
            format(PATTERN_1, &p).unwrap(),
            "Tool/(2006) 10,000 Days [FLAC]/0101. Tool - Vicarious"
        );
    }

    // Pattern 2: $if2(%label%,%album artist%)/%album% '['%codec%']'/[$num(%discnumber%,2)][%tracknumber%. ][%artist% - ]%title%
    //
    // Logic: use label if available, fall back to album artist. Rest is same structure.

    const PATTERN_2: &str = "$if2(%label%,%album artist%)/%album% '['%codec%']'/[$num(%discnumber%,2)][%tracknumber%. ][%artist% - ]%title%";

    #[test]
    fn pattern2_with_label() {
        let p = provider(&[
            ("label", "Warp Records"),
            ("album artist", "Aphex Twin"),
            ("album", "Selected Ambient Works 85-92"),
            ("codec", "FLAC"),
            ("tracknumber", "01"),
            ("artist", "Aphex Twin"),
            ("title", "Xtal"),
        ]);
        assert_eq!(
            format(PATTERN_2, &p).unwrap(),
            "Warp Records/Selected Ambient Works 85-92 [FLAC]/01. Aphex Twin - Xtal"
        );
    }

    #[test]
    fn pattern2_no_label_fallback_to_artist() {
        let p = provider(&[
            ("album artist", "Aphex Twin"),
            ("album", "Selected Ambient Works 85-92"),
            ("codec", "FLAC"),
            ("tracknumber", "01"),
            ("artist", "Aphex Twin"),
            ("title", "Xtal"),
        ]);
        assert_eq!(
            format(PATTERN_2, &p).unwrap(),
            "Aphex Twin/Selected Ambient Works 85-92 [FLAC]/01. Aphex Twin - Xtal"
        );
    }

    #[test]
    fn pattern2_with_disc_number() {
        let p = provider(&[
            ("album artist", "Tool"),
            ("album", "10,000 Days"),
            ("codec", "FLAC"),
            ("discnumber", "2"),
            ("tracknumber", "01"),
            ("artist", "Tool"),
            ("title", "Wings for Marie"),
        ]);
        assert_eq!(
            format(PATTERN_2, &p).unwrap(),
            "Tool/10,000 Days [FLAC]/0201. Tool - Wings for Marie"
        );
    }

    #[test]
    fn pattern2_minimal_metadata() {
        let p = provider(&[
            ("album artist", "Unknown"),
            ("album", "Untitled"),
            ("codec", "MP3"),
            ("title", "Track 1"),
        ]);
        // No disc, no tracknumber, no artist → all conditionals skipped
        assert_eq!(
            format(PATTERN_2, &p).unwrap(),
            "Unknown/Untitled [MP3]/Track 1"
        );
    }

    // ======================================================================
    // Additional edge cases
    // ======================================================================

    #[test]
    fn nested_function_in_function() {
        let p = provider(&[("artist", "aphex twin")]);
        assert_eq!(
            format("$upper($left(%artist%,5))", &p).unwrap(),
            "APHEX"
        );
    }

    #[test]
    fn multiple_conditionals_adjacent() {
        let p = provider(&[
            ("artist", "Radiohead"),
            ("album", "OK Computer"),
        ]);
        assert_eq!(
            format("[%artist%][ - %album%]", &p).unwrap(),
            "Radiohead - OK Computer"
        );
    }

    #[test]
    fn multiple_conditionals_partial() {
        let p = provider(&[("artist", "Radiohead")]);
        assert_eq!(
            format("[%artist%][ - %album%][ (%date%)]", &p).unwrap(),
            "Radiohead"
        );
    }

    #[test]
    fn empty_format_string() {
        let p = provider(&[]);
        assert_eq!(format("", &p).unwrap(), "");
    }

    #[test]
    fn only_literals() {
        let p = provider(&[]);
        assert_eq!(format("hello world", &p).unwrap(), "hello world");
    }

    #[test]
    fn quoted_brackets_outside_conditional() {
        let p = provider(&[("codec", "FLAC")]);
        assert_eq!(
            format("'['%codec%']'", &p).unwrap(),
            "[FLAC]"
        );
    }

    #[test]
    fn if_with_stricmp_nested_in_conditional() {
        // $if inside a conditional: the conditional tracks field resolution
        let p = provider(&[
            ("album artist", "Radiohead"),
            ("date", "1997"),
        ]);
        assert_eq!(
            format(
                "[$if($stricmp(%album artist%,Various Artists),,'('$left(%date%,4)')' )]%album artist%",
                &p
            )
            .unwrap(),
            "(1997) Radiohead"
        );
    }

    #[test]
    fn stricmp_in_if_empty_then_branch() {
        // The key pattern: $if($stricmp(x,y),,alternative) — empty then branch
        let p = provider(&[("album artist", "Radiohead")]);
        assert_eq!(
            format("$if($stricmp(%album artist%,Radiohead),,not radiohead)", &p).unwrap(),
            "" // stricmp matches → takes empty then branch
        );
    }

    #[test]
    fn stricmp_in_if_else_branch() {
        let p = provider(&[("album artist", "Aphex Twin")]);
        assert_eq!(
            format("$if($stricmp(%album artist%,Radiohead),,not radiohead)", &p).unwrap(),
            "not radiohead" // stricmp doesn't match → takes else branch
        );
    }

    #[test]
    fn if2_both_empty() {
        let p = provider(&[]);
        // Both %label% and %album artist% missing → $if2 returns None → function returns None → empty
        assert_eq!(
            format("$if2(%label%,%album artist%)", &p).unwrap(),
            ""
        );
    }

    #[test]
    fn organize_standard_layout() {
        // Standard organize pattern from docs/format-strings.md
        let p = provider(&[
            ("album artist", "Aphex Twin"),
            ("date", "1999"),
            ("album", "Windowlicker EP"),
            ("tracknumber", "01"),
            ("title", "Windowlicker"),
        ]);
        assert_eq!(
            format("%album artist%/(%date%) %album%/%tracknumber%. %title%", &p).unwrap(),
            "Aphex Twin/(1999) Windowlicker EP/01. Windowlicker"
        );
    }

    #[test]
    fn organize_multi_disc() {
        // Multi-disc organize pattern from docs/format-strings.md
        let p = provider(&[
            ("album artist", "Tool"),
            ("date", "2006"),
            ("album", "10,000 Days"),
            ("discnumber", "1"),
            ("tracknumber", "01"),
            ("title", "Vicarious"),
        ]);
        assert_eq!(
            format(
                "%album artist%/[(%date%) ]%album%/[%discnumber%-]%tracknumber%. %title%",
                &p
            )
            .unwrap(),
            "Tool/(2006) 10,000 Days/1-01. Vicarious"
        );
    }

    #[test]
    fn organize_multi_disc_no_disc() {
        let p = provider(&[
            ("album artist", "Tool"),
            ("date", "2006"),
            ("album", "10,000 Days"),
            ("tracknumber", "01"),
            ("title", "Vicarious"),
        ]);
        assert_eq!(
            format(
                "%album artist%/[(%date%) ]%album%/[%discnumber%-]%tracknumber%. %title%",
                &p
            )
            .unwrap(),
            "Tool/(2006) 10,000 Days/01. Vicarious"
        );
    }

    #[test]
    fn organize_if2_fallback() {
        // Fallback artist pattern from docs/format-strings.md
        let p = provider(&[
            ("artist", "Flying Lotus"),
            ("album", "Cosmogramma"),
            ("tracknumber", "01"),
            ("title", "Clock Catcher"),
        ]);
        assert_eq!(
            format(
                "$if2(%album artist%,%artist%)/%album%/%tracknumber%. %title%",
                &p
            )
            .unwrap(),
            "Flying Lotus/Cosmogramma/01. Clock Catcher"
        );
    }

    #[test]
    fn organize_genre_based() {
        let p = provider(&[
            ("genre", "Electronic"),
            ("album artist", "Aphex Twin"),
            ("album", "Windowlicker EP"),
            ("tracknumber", "01"),
            ("title", "Windowlicker"),
        ]);
        assert_eq!(
            format(
                "[%genre%/]%album artist%/%album%/%tracknumber%. %title%",
                &p
            )
            .unwrap(),
            "Electronic/Aphex Twin/Windowlicker EP/01. Windowlicker"
        );
    }

    #[test]
    fn organize_genre_based_no_genre() {
        let p = provider(&[
            ("album artist", "Aphex Twin"),
            ("album", "Windowlicker EP"),
            ("tracknumber", "01"),
            ("title", "Windowlicker"),
        ]);
        assert_eq!(
            format(
                "[%genre%/]%album artist%/%album%/%tracknumber%. %title%",
                &p
            )
            .unwrap(),
            "Aphex Twin/Windowlicker EP/01. Windowlicker"
        );
    }
}
