#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    Literal(String),
    Field(String),
    Conditional(Vec<Token>),
    Function { name: String, args: Vec<Vec<Token>> },
}

#[derive(Debug, thiserror::Error)]
pub enum FormatError {
    #[error("unclosed field at position {0}")]
    UnclosedField(usize),
    #[error("unclosed conditional at position {0}")]
    UnclosedConditional(usize),
    #[error("unclosed function at position {0}")]
    UnclosedFunction(usize),
    #[error("unexpected character '{0}' at position {1}")]
    UnexpectedChar(char, usize),
}

pub fn parse(input: &str) -> Result<Vec<Token>, FormatError> {
    let chars: Vec<char> = input.chars().collect();
    let (tokens, _) = parse_tokens(&chars, 0, &[])?;
    Ok(tokens)
}

/// Parse tokens until we hit a stop character or end of input.
/// Returns the parsed tokens and the position after the stop character (or end).
fn parse_tokens(
    chars: &[char],
    start: usize,
    stop_chars: &[char],
) -> Result<(Vec<Token>, usize), FormatError> {
    let mut tokens = Vec::new();
    let mut pos = start;
    let mut literal = String::new();

    while pos < chars.len() {
        let ch = chars[pos];

        if stop_chars.contains(&ch) {
            if !literal.is_empty() {
                tokens.push(Token::Literal(literal));
            }
            return Ok((tokens, pos));
        }

        match ch {
            '%' => {
                if !literal.is_empty() {
                    tokens.push(Token::Literal(std::mem::take(&mut literal)));
                }
                pos += 1;
                let field_start = pos;
                while pos < chars.len() && chars[pos] != '%' {
                    pos += 1;
                }
                if pos >= chars.len() {
                    return Err(FormatError::UnclosedField(field_start - 1));
                }
                let name: String = chars[field_start..pos].iter().collect();
                tokens.push(Token::Field(name));
                pos += 1;
            }
            '[' => {
                if !literal.is_empty() {
                    tokens.push(Token::Literal(std::mem::take(&mut literal)));
                }
                let bracket_pos = pos;
                pos += 1;
                let (inner, end) = parse_tokens(chars, pos, &[']'])?;
                if end >= chars.len() || chars[end] != ']' {
                    return Err(FormatError::UnclosedConditional(bracket_pos));
                }
                tokens.push(Token::Conditional(inner));
                pos = end + 1;
            }
            '$' => {
                if !literal.is_empty() {
                    tokens.push(Token::Literal(std::mem::take(&mut literal)));
                }
                pos += 1;
                let name_start = pos;
                while pos < chars.len() && chars[pos].is_alphanumeric()
                    || pos < chars.len() && chars[pos] == '_'
                {
                    pos += 1;
                }
                let name: String = chars[name_start..pos].iter().collect();
                if pos >= chars.len() || chars[pos] != '(' {
                    return Err(FormatError::UnclosedFunction(name_start - 1));
                }
                pos += 1; // skip '('
                let args = parse_function_args(chars, pos, name_start - 1)?;
                let mut end = pos;
                // Find the matching ')' respecting nesting
                let mut depth = 1;
                while end < chars.len() && depth > 0 {
                    match chars[end] {
                        '(' => depth += 1,
                        ')' => depth -= 1,
                        _ => {}
                    }
                    if depth > 0 {
                        end += 1;
                    }
                }
                if depth != 0 {
                    return Err(FormatError::UnclosedFunction(name_start - 1));
                }
                pos = end + 1;
                tokens.push(Token::Function { name, args });
            }
            '\'' => {
                if !literal.is_empty() {
                    tokens.push(Token::Literal(std::mem::take(&mut literal)));
                }
                pos += 1;
                let mut quoted = String::new();
                while pos < chars.len() && chars[pos] != '\'' {
                    quoted.push(chars[pos]);
                    pos += 1;
                }
                if pos < chars.len() {
                    pos += 1; // skip closing quote
                }
                tokens.push(Token::Literal(quoted));
            }
            _ => {
                literal.push(ch);
                pos += 1;
            }
        }
    }

    if !literal.is_empty() {
        tokens.push(Token::Literal(literal));
    }

    if !stop_chars.is_empty() {
        // We reached end of input but expected a stop character
        if stop_chars.contains(&']') {
            return Err(FormatError::UnclosedConditional(start.saturating_sub(1)));
        }
        if stop_chars.contains(&')') {
            return Err(FormatError::UnclosedFunction(start.saturating_sub(1)));
        }
    }

    Ok((tokens, pos))
}

/// Parse comma-separated function arguments, respecting nesting.
fn parse_function_args(
    chars: &[char],
    start: usize,
    func_pos: usize,
) -> Result<Vec<Vec<Token>>, FormatError> {
    let mut args = Vec::new();
    let mut pos = start;

    loop {
        let (arg_tokens, end) = parse_tokens(chars, pos, &[',', ')'])?;
        args.push(arg_tokens);

        if end >= chars.len() {
            return Err(FormatError::UnclosedFunction(func_pos));
        }

        if chars[end] == ')' {
            break;
        }
        // comma — continue to next arg
        pos = end + 1;
    }

    Ok(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_field() {
        assert_eq!(
            parse("%title%").unwrap(),
            vec![Token::Field("title".into())]
        );
    }

    #[test]
    fn literal_and_field() {
        assert_eq!(
            parse("Track: %title%").unwrap(),
            vec![
                Token::Literal("Track: ".into()),
                Token::Field("title".into())
            ]
        );
    }

    #[test]
    fn conditional() {
        assert_eq!(
            parse("[%artist% - ]%title%").unwrap(),
            vec![
                Token::Conditional(vec![
                    Token::Field("artist".into()),
                    Token::Literal(" - ".into()),
                ]),
                Token::Field("title".into()),
            ]
        );
    }

    #[test]
    fn function_with_field_arg() {
        assert_eq!(
            parse("$left(%date%,4)").unwrap(),
            vec![Token::Function {
                name: "left".into(),
                args: vec![
                    vec![Token::Field("date".into())],
                    vec![Token::Literal("4".into())]
                ],
            }]
        );
    }

    #[test]
    fn nested_conditional_function() {
        let result = parse("[$if(%genre%,%genre%,Unknown)]").unwrap();
        assert_eq!(
            result,
            vec![Token::Conditional(vec![Token::Function {
                name: "if".into(),
                args: vec![
                    vec![Token::Field("genre".into())],
                    vec![Token::Field("genre".into())],
                    vec![Token::Literal("Unknown".into())],
                ],
            }])]
        );
    }

    #[test]
    fn quoted_literal() {
        assert_eq!(
            parse("'hello'").unwrap(),
            vec![Token::Literal("hello".into())]
        );
    }

    #[test]
    fn unclosed_field() {
        assert!(matches!(
            parse("%title"),
            Err(FormatError::UnclosedField(_))
        ));
    }

    #[test]
    fn unclosed_conditional() {
        assert!(matches!(
            parse("[%title%"),
            Err(FormatError::UnclosedConditional(_))
        ));
    }

    #[test]
    fn unclosed_function() {
        assert!(matches!(
            parse("$left(%title%,3"),
            Err(FormatError::UnclosedFunction(_))
        ));
    }

    #[test]
    fn plain_literal() {
        assert_eq!(
            parse("hello world").unwrap(),
            vec![Token::Literal("hello world".into())]
        );
    }

    #[test]
    fn nested_conditionals() {
        let result = parse("[%artist%[ (%date%)]]").unwrap();
        assert_eq!(
            result,
            vec![Token::Conditional(vec![
                Token::Field("artist".into()),
                Token::Conditional(vec![
                    Token::Literal(" (".into()),
                    Token::Field("date".into()),
                    Token::Literal(")".into()),
                ]),
            ])]
        );
    }
}
