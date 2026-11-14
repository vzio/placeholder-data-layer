//! CPF and CNPJ check digits, written from the published mod-11 rule rather
//! than shared with the engine, so a corpus judgement of validity is
//! independent of the code it judges.

use serde::Deserialize;

use super::rng::Rng;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Document {
    Cpf,
    Cnpj,
}

impl Document {
    pub fn width(self) -> usize {
        match self {
            Document::Cpf => 11,
            Document::Cnpj => 14,
        }
    }

    /// The two check digits for a base of `width - 2` digits.
    pub fn check_digits(self, base: &[u8]) -> [u8; 2] {
        match self {
            Document::Cpf => {
                let first = cpf_digit(base, 10);
                let mut extended = base.to_vec();
                extended.push(first);
                [first, cpf_digit(&extended, 11)]
            }
            Document::Cnpj => {
                let first = cnpj_digit(base, &[5, 4, 3, 2, 9, 8, 7, 6, 5, 4, 3, 2]);
                let mut extended = base.to_vec();
                extended.push(first);
                [
                    first,
                    cnpj_digit(&extended, &[6, 5, 4, 3, 2, 9, 8, 7, 6, 5, 4, 3, 2]),
                ]
            }
        }
    }

    /// Whether `text` is a valid document once punctuation (`.`, `-`, `/`,
    /// spaces) is dropped; any other character makes it invalid. An
    /// all-digit text shorter than the width is read as an integer that lost
    /// its leading zeros; anything else must carry exactly `width` digits.
    pub fn is_valid(self, text: &str) -> bool {
        let trimmed = text.trim();
        if !trimmed
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '/' | ' '))
        {
            return false;
        }
        let mut digits: Vec<u8> = trimmed
            .bytes()
            .filter(u8::is_ascii_digit)
            .map(|b| b - b'0')
            .collect();
        let all_digits = !trimmed.is_empty() && trimmed.bytes().all(|b| b.is_ascii_digit());
        if all_digits && digits.len() < self.width() {
            let mut padded = vec![0; self.width() - digits.len()];
            padded.extend(digits);
            digits = padded;
        }
        if digits.len() != self.width() || digits.iter().all(|d| *d == digits[0]) {
            return false;
        }
        let (base, check) = digits.split_at(self.width() - 2);
        self.check_digits(base) == [check[0], check[1]]
    }

    /// A valid document's digits, never one digit repeated.
    pub fn generate(self, rng: &mut Rng) -> Vec<u8> {
        loop {
            let mut digits: Vec<u8> = (0..self.width() - 2).map(|_| rng.below(10) as u8).collect();
            if self == Document::Cnpj {
                digits[8..12].copy_from_slice(&[0, 0, 0, 1]);
            }
            if digits.iter().all(|d| *d == digits[0]) {
                continue;
            }
            let check = self.check_digits(&digits);
            digits.extend(check);
            return digits;
        }
    }

    /// A document of the right width whose check digits are wrong.
    pub fn generate_invalid(self, rng: &mut Rng) -> Vec<u8> {
        let mut digits = self.generate(rng);
        let last = digits.len() - 1;
        digits[last] = (digits[last] + 1 + rng.below(9) as u8) % 10;
        digits
    }

    pub fn masked(self, digits: &[u8]) -> String {
        let d = text_of(digits);
        match self {
            Document::Cpf => format!("{}.{}.{}-{}", &d[0..3], &d[3..6], &d[6..9], &d[9..11]),
            Document::Cnpj => format!(
                "{}.{}.{}/{}-{}",
                &d[0..2],
                &d[2..5],
                &d[5..8],
                &d[8..12],
                &d[12..14]
            ),
        }
    }
}

pub fn text_of(digits: &[u8]) -> String {
    digits.iter().map(|d| char::from(b'0' + d)).collect()
}

/// The digits as an integer would print them: leading zeros dropped.
pub fn without_leading_zeros(digits: &[u8]) -> String {
    let text = text_of(digits);
    let trimmed = text.trim_start_matches('0');
    if trimmed.is_empty() {
        "0".to_owned()
    } else {
        trimmed.to_owned()
    }
}

fn cpf_digit(digits: &[u8], first_weight: u32) -> u8 {
    let sum: u32 = digits
        .iter()
        .zip((2..=first_weight).rev())
        .map(|(d, w)| u32::from(*d) * w)
        .sum();
    let rest = (sum * 10) % 11;
    if rest == 10 { 0 } else { rest as u8 }
}

fn cnpj_digit(digits: &[u8], weights: &[u32]) -> u8 {
    let sum: u32 = digits
        .iter()
        .zip(weights)
        .map(|(d, w)| u32::from(*d) * w)
        .sum();
    let rest = sum % 11;
    if rest < 2 { 0 } else { (11 - rest) as u8 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn published_examples_validate() {
        assert!(Document::Cpf.is_valid("111.444.777-35"));
        assert!(Document::Cpf.is_valid("11144477735"));
        assert!(Document::Cnpj.is_valid("11.222.333/0001-81"));
        assert!(Document::Cnpj.is_valid("11222333000181"));
    }

    #[test]
    fn hand_computed_check_digits_match() {
        // 1*10+1*9+1*8+4*7+4*6+4*5+7*4+7*3+7*2 = 162; 1620 % 11 = 3.
        assert_eq!(
            Document::Cpf.check_digits(&[1, 1, 1, 4, 4, 4, 7, 7, 7]),
            [3, 5]
        );
        assert_eq!(
            Document::Cnpj.check_digits(&[1, 1, 2, 2, 2, 3, 3, 3, 0, 0, 0, 1]),
            [8, 1]
        );
    }

    #[test]
    fn wrong_digits_repeated_digits_and_wrong_widths_are_invalid() {
        assert!(!Document::Cpf.is_valid("111.444.777-36"));
        assert!(!Document::Cpf.is_valid("111.111.111-11"));
        assert!(!Document::Cpf.is_valid("1114447773"));
        assert!(!Document::Cnpj.is_valid("11.222.333/0001-82"));
        assert!(!Document::Cpf.is_valid(""));
        assert!(!Document::Cpf.is_valid("111.444.7x77-35"));
    }

    #[test]
    fn an_integer_that_dropped_leading_zeros_is_padded_back() {
        let mut rng = Rng::new(11);
        let mut digits = Document::Cnpj.generate(&mut rng);
        digits[0] = 0;
        let check = Document::Cnpj.check_digits(&digits[..12]);
        digits[12..].copy_from_slice(&check);
        assert!(Document::Cnpj.is_valid(&without_leading_zeros(&digits)));
    }

    #[test]
    fn generated_documents_validate_and_invalid_ones_do_not() {
        let mut rng = Rng::new(5);
        for document in [Document::Cpf, Document::Cnpj] {
            for _ in 0..200 {
                assert!(document.is_valid(&document.masked(&document.generate(&mut rng))));
                assert!(!document.is_valid(&text_of(&document.generate_invalid(&mut rng))));
            }
        }
    }
}
