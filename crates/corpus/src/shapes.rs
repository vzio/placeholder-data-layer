//! Named value shapes, written for the corpus rather than borrowed from the
//! engine's classifier.

use std::sync::OnceLock;

use regex::Regex;
use serde::Deserialize;

use crate::CorpusError;

/// Every named shape and its pattern, unanchored.
pub const NAMED: &[(&str, &str)] = &[
    ("cpf_masked", r"\d{3}\.\d{3}\.\d{3}-\d{2}"),
    ("cpf_digits", r"\d{11}"),
    ("cnpj_masked", r"\d{2}\.\d{3}\.\d{3}/\d{4}-\d{2}"),
    ("cnpj_digits", r"\d{14}"),
    ("cep_masked", r"\d{5}-\d{3}"),
    ("cep_digits", r"\d{8}"),
    ("phone_br_masked", r"\(\d{2}\) ?\d{4,5}-\d{4}"),
    ("phone_br_digits", r"\d{10,11}"),
    ("phone_br_e164", r"\+55\d{10,11}"),
    ("email", r"[^@\s]+@[^@\s]+\.[^@\s]+"),
    ("date_iso", r"\d{4}-\d{2}-\d{2}"),
    ("date_br", r"\d{2}/\d{2}/\d{4}"),
    (
        "uuid",
        r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}",
    ),
];

/// A named shape, or an escape-hatch regular expression.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum ShapeRef {
    Named(String),
    Regex { regex: String },
}

impl ShapeRef {
    pub fn label(&self) -> String {
        match self {
            ShapeRef::Named(name) => name.clone(),
            ShapeRef::Regex { regex } => format!("/{regex}/"),
        }
    }

    fn pattern(&self) -> Result<&str, CorpusError> {
        match self {
            ShapeRef::Named(name) => NAMED
                .iter()
                .find(|(named, _)| named == name)
                .map(|(_, pattern)| *pattern)
                .ok_or_else(|| CorpusError(format!("unknown shape `{name}`"))),
            ShapeRef::Regex { regex } => Ok(regex),
        }
    }

    pub fn compile(&self) -> Result<Shape, CorpusError> {
        let pattern = self.pattern()?;
        let build = |p: String| Regex::new(&p).map_err(|e| CorpusError(e.to_string()));
        Ok(Shape {
            whole: build(format!("^(?:{pattern})$"))?,
            anywhere: build(pattern.to_owned())?,
        })
    }
}

pub struct Shape {
    whole: Regex,
    anywhere: Regex,
}

impl Shape {
    /// The whole value, untrimmed, is this shape.
    pub fn is(&self, text: &str) -> bool {
        self.whole.is_match(text)
    }

    /// Some part of the value is this shape and is not glued to further
    /// digits on either side.
    pub fn occurs_in(&self, text: &str) -> bool {
        self.anywhere.find_iter(text).any(|m| {
            let before = text[..m.start()].chars().next_back();
            let after = text[m.end()..].chars().next();
            !before.is_some_and(|c| c.is_ascii_digit())
                && !after.is_some_and(|c| c.is_ascii_digit())
        })
    }
}

/// The first named shape the whole value is, by table order.
pub fn classify(text: &str) -> Option<&'static str> {
    static COMPILED: OnceLock<Vec<(&'static str, Shape)>> = OnceLock::new();
    COMPILED
        .get_or_init(|| {
            NAMED
                .iter()
                .filter_map(|(name, _)| {
                    let shape = ShapeRef::Named((*name).to_owned()).compile().ok()?;
                    Some((*name, shape))
                })
                .collect()
        })
        .iter()
        .find(|(_, shape)| shape.is(text))
        .map(|(name, _)| *name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(name: &str) -> Shape {
        match ShapeRef::Named(name.to_owned()).compile() {
            Ok(shape) => shape,
            Err(e) => panic!("{e}"),
        }
    }

    #[test]
    fn named_shapes_match_their_canonical_examples_whole() {
        let cases = [
            ("cpf_masked", "111.444.777-35"),
            ("cpf_digits", "11144477735"),
            ("cnpj_masked", "11.222.333/0001-81"),
            ("cnpj_digits", "11222333000181"),
            ("cep_masked", "01310-100"),
            ("cep_digits", "01310100"),
            ("phone_br_masked", "(11) 98765-4321"),
            ("phone_br_masked", "(11) 3265-4321"),
            ("phone_br_digits", "11987654321"),
            ("phone_br_e164", "+5511987654321"),
            ("email", "ana.silva@gmail.com"),
            ("date_iso", "2023-02-28"),
            ("date_br", "28/02/2023"),
            ("uuid", "123e4567-e89b-12d3-a456-426614174000"),
        ];
        for (name, example) in cases {
            assert!(named(name).is(example), "{name} should match {example}");
        }
        assert!(!named("cpf_masked").is(" 111.444.777-35"));
        assert!(!named("email").is("ana.silvagmail.com"));
    }

    #[test]
    fn occurrence_inside_text_respects_digit_boundaries() {
        let cpf = named("cpf_digits");
        assert!(cpf.occurs_in("cpf 11144477735 ok"));
        assert!(!cpf.occurs_in("pedido 111444777351"));
        assert!(named("phone_br_masked").occurs_in("ligar (11) 98765-4321 hoje"));
    }

    #[test]
    fn a_regex_shape_and_an_unknown_name_compile_as_expected() {
        let shape = ShapeRef::Regex {
            regex: r"\d{12,13}".to_owned(),
        };
        assert!(shape.compile().is_ok_and(|s| s.is("1222333000181")));
        assert!(ShapeRef::Named("nope".to_owned()).compile().is_err());
        assert_eq!(classify("01310-100"), Some("cep_masked"));
        assert_eq!(classify("hello"), None);
    }
}
