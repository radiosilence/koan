use crate::format::functions::call_function;
use crate::format::parser::Token;

pub trait MetadataProvider {
    fn get_field(&self, name: &str) -> Option<String>;
}

impl<F: Fn(&str) -> Option<String>> MetadataProvider for F {
    fn get_field(&self, name: &str) -> Option<String> {
        self(name)
    }
}

/// Evaluate tokens against a metadata provider.
pub fn evaluate(tokens: &[Token], provider: &dyn MetadataProvider) -> String {
    let (result, _) = eval_inner(tokens, provider);
    result
}

/// Returns (output_string, all_fields_resolved).
fn eval_inner(tokens: &[Token], provider: &dyn MetadataProvider) -> (String, bool) {
    let mut output = String::new();
    let mut all_resolved = true;

    for token in tokens {
        match token {
            Token::Literal(s) => output.push_str(s),
            Token::Field(name) => match provider.get_field(name) {
                Some(val) => output.push_str(&val),
                None => {
                    all_resolved = false;
                }
            },
            Token::Conditional(inner) => {
                let (inner_output, inner_resolved) = eval_inner(inner, provider);
                if inner_resolved {
                    output.push_str(&inner_output);
                }
                // A failed conditional doesn't poison the parent — it just produces ""
            }
            Token::Function { name, args } => {
                let mut eval_args = Vec::new();
                let mut func_resolved = true;
                for arg_tokens in args {
                    let (arg_val, arg_ok) = eval_inner(arg_tokens, provider);
                    if !arg_ok {
                        func_resolved = false;
                    }
                    eval_args.push(arg_val);
                }
                if !func_resolved {
                    all_resolved = false;
                }
                if let Some(result) = call_function(name, &eval_args) {
                    output.push_str(&result);
                }
            }
        }
    }

    (output, all_resolved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_provider(data: &[(&str, &str)]) -> HashMap<String, String> {
        data.iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    impl MetadataProvider for HashMap<String, String> {
        fn get_field(&self, name: &str) -> Option<String> {
            self.get(name).cloned()
        }
    }

    #[test]
    fn basic_field() {
        let provider = make_provider(&[("title", "Song")]);
        let tokens = vec![Token::Field("title".into())];
        assert_eq!(evaluate(&tokens, &provider), "Song");
    }

    #[test]
    fn missing_field() {
        let provider = make_provider(&[]);
        let tokens = vec![Token::Field("title".into())];
        assert_eq!(evaluate(&tokens, &provider), "");
    }

    #[test]
    fn conditional_present() {
        let provider = make_provider(&[("artist", "Radiohead")]);
        let tokens = vec![
            Token::Conditional(vec![
                Token::Field("artist".into()),
                Token::Literal(" - ".into()),
            ]),
            Token::Literal("OK Computer".into()),
        ];
        assert_eq!(evaluate(&tokens, &provider), "Radiohead - OK Computer");
    }

    #[test]
    fn conditional_missing() {
        let provider = make_provider(&[]);
        let tokens = vec![
            Token::Conditional(vec![
                Token::Field("artist".into()),
                Token::Literal(" - ".into()),
            ]),
            Token::Literal("OK Computer".into()),
        ];
        assert_eq!(evaluate(&tokens, &provider), "OK Computer");
    }

    #[test]
    fn nested_conditional() {
        let provider = make_provider(&[("artist", "Radiohead"), ("date", "1997")]);
        let tokens = vec![Token::Conditional(vec![
            Token::Field("artist".into()),
            Token::Conditional(vec![
                Token::Literal(" (".into()),
                Token::Field("date".into()),
                Token::Literal(")".into()),
            ]),
        ])];
        assert_eq!(evaluate(&tokens, &provider), "Radiohead (1997)");
    }

    #[test]
    fn nested_conditional_inner_missing() {
        let provider = make_provider(&[("artist", "Radiohead")]);
        let tokens = vec![Token::Conditional(vec![
            Token::Field("artist".into()),
            Token::Conditional(vec![
                Token::Literal(" (".into()),
                Token::Field("date".into()),
                Token::Literal(")".into()),
            ]),
        ])];
        // Inner conditional fails (date missing), outer still works (artist present)
        assert_eq!(evaluate(&tokens, &provider), "Radiohead");
    }

    #[test]
    fn function_in_conditional() {
        let provider = make_provider(&[("genre", "Rock")]);
        let tokens = vec![Token::Conditional(vec![Token::Function {
            name: "if".into(),
            args: vec![
                vec![Token::Field("genre".into())],
                vec![Token::Field("genre".into())],
                vec![Token::Literal("Unknown".into())],
            ],
        }])];
        assert_eq!(evaluate(&tokens, &provider), "Rock");
    }

    #[test]
    fn function_in_conditional_field_missing() {
        let provider = make_provider(&[]);
        let tokens = vec![Token::Conditional(vec![Token::Function {
            name: "if".into(),
            args: vec![
                vec![Token::Field("genre".into())],
                vec![Token::Field("genre".into())],
                vec![Token::Literal("Unknown".into())],
            ],
        }])];
        // genre is missing, so the conditional fails entirely
        assert_eq!(evaluate(&tokens, &provider), "");
    }

    #[test]
    fn literal_passthrough() {
        let provider = make_provider(&[]);
        let tokens = vec![Token::Literal("hello".into())];
        assert_eq!(evaluate(&tokens, &provider), "hello");
    }

    #[test]
    fn multiple_fields() {
        let provider = make_provider(&[("artist", "Radiohead"), ("title", "Creep")]);
        let tokens = vec![
            Token::Field("artist".into()),
            Token::Literal(" - ".into()),
            Token::Field("title".into()),
        ];
        assert_eq!(evaluate(&tokens, &provider), "Radiohead - Creep");
    }

    #[test]
    fn missing_field_produces_empty() {
        let provider = make_provider(&[("title", "Creep")]);
        let tokens = vec![
            Token::Field("artist".into()),
            Token::Literal(" - ".into()),
            Token::Field("title".into()),
        ];
        // Missing field produces empty string inline (not inside conditional)
        assert_eq!(evaluate(&tokens, &provider), " - Creep");
    }

    #[test]
    fn conditional_does_not_poison_parent() {
        let provider = make_provider(&[("title", "Creep")]);
        let tokens = vec![
            Token::Conditional(vec![
                Token::Field("artist".into()),
                Token::Literal(" - ".into()),
            ]),
            Token::Field("title".into()),
        ];
        // Failed conditional produces "", rest of output is fine
        assert_eq!(evaluate(&tokens, &provider), "Creep");
    }

    #[test]
    fn multiple_failed_conditionals() {
        let provider = make_provider(&[("title", "Creep")]);
        let tokens = vec![
            Token::Conditional(vec![Token::Field("artist".into())]),
            Token::Conditional(vec![Token::Field("album".into())]),
            Token::Conditional(vec![Token::Field("date".into())]),
            Token::Field("title".into()),
        ];
        assert_eq!(evaluate(&tokens, &provider), "Creep");
    }

    #[test]
    fn deeply_nested_conditionals() {
        let provider = make_provider(&[("a", "1"), ("b", "2"), ("c", "3")]);
        let tokens = vec![Token::Conditional(vec![
            Token::Field("a".into()),
            Token::Conditional(vec![
                Token::Field("b".into()),
                Token::Conditional(vec![Token::Field("c".into())]),
            ]),
        ])];
        assert_eq!(evaluate(&tokens, &provider), "123");
    }

    #[test]
    fn function_with_empty_arg() {
        let provider = make_provider(&[]);
        // Simulate $if(x,,y) where arg 1 is empty
        let tokens = vec![Token::Function {
            name: "if".into(),
            args: vec![
                vec![Token::Literal("truthy".into())],
                vec![], // empty then branch
                vec![Token::Literal("else".into())],
            ],
        }];
        // "truthy" is non-empty → takes then branch → ""
        assert_eq!(evaluate(&tokens, &provider), "");
    }

    #[test]
    fn stricmp_with_if_empty_then() {
        let provider = make_provider(&[("x", "hello")]);
        // $if($stricmp(%x%,hello),,not hello)
        let tokens = vec![Token::Function {
            name: "if".into(),
            args: vec![
                vec![Token::Function {
                    name: "stricmp".into(),
                    args: vec![
                        vec![Token::Field("x".into())],
                        vec![Token::Literal("hello".into())],
                    ],
                }],
                vec![], // empty then
                vec![Token::Literal("not hello".into())],
            ],
        }];
        // stricmp("hello", "hello") → "1" (truthy) → takes empty then → ""
        assert_eq!(evaluate(&tokens, &provider), "");
    }

    #[test]
    fn stricmp_with_if_else() {
        let provider = make_provider(&[("x", "world")]);
        let tokens = vec![Token::Function {
            name: "if".into(),
            args: vec![
                vec![Token::Function {
                    name: "stricmp".into(),
                    args: vec![
                        vec![Token::Field("x".into())],
                        vec![Token::Literal("hello".into())],
                    ],
                }],
                vec![],
                vec![Token::Literal("not hello".into())],
            ],
        }];
        // stricmp("world", "hello") → "" (falsy) → takes else → "not hello"
        assert_eq!(evaluate(&tokens, &provider), "not hello");
    }

    #[test]
    fn conditional_with_only_literals_always_renders() {
        let provider = make_provider(&[]);
        let tokens = vec![Token::Conditional(vec![Token::Literal("always".into())])];
        // No fields to fail → all_resolved stays true → renders
        assert_eq!(evaluate(&tokens, &provider), "always");
    }

    #[test]
    fn conditional_with_function_returning_empty() {
        let provider = make_provider(&[("x", "nope")]);
        let tokens = vec![Token::Conditional(vec![Token::Function {
            name: "stricmp".into(),
            args: vec![
                vec![Token::Field("x".into())],
                vec![Token::Literal("hello".into())],
            ],
        }])];
        // stricmp returns "" but all fields resolved → conditional renders ""
        assert_eq!(evaluate(&tokens, &provider), "");
    }
}
