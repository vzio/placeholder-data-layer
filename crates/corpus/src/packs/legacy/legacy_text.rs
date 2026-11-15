//! How old systems print text, numbers and dates: Windows-1252 mojibake,
//! fixed-width padding, dates as `YYYYMMDD` integers, pt-BR decimals and the
//! spellings a spreadsheet gives a long number or a date.

use crate::generate::rng::Rng;

/// The characters Windows-1252 assigns to the bytes `0x80..=0x9F`; the five
/// it leaves undefined decode as the Latin-1 control at that code point.
const CP1252_HIGH: [char; 32] = [
    '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8D}', 'Ž', '\u{8F}',
    '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9D}', 'ž', 'Ÿ',
];

/// UTF-8 bytes read back as Windows-1252: `JOÃO` becomes `JOÃƒO`,
/// `Conceição` becomes `ConceiÃ§Ã£o`.
pub fn mojibake_cp1252(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 2);
    for c in text.chars() {
        if c.is_ascii() {
            out.push(c);
            continue;
        }
        let mut buffer = [0u8; 4];
        for byte in c.encode_utf8(&mut buffer).bytes() {
            match byte {
                0x80..=0x9F => out.push(CP1252_HIGH[usize::from(byte - 0x80)]),
                _ => out.push(char::from(byte)),
            }
        }
    }
    out
}

/// Upper case without diacritics, the way a terminal-era system keyed
/// names in: `Conceição` becomes `CONCEICAO`.
pub fn upper_ascii(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            'á' | 'à' | 'â' | 'ã' | 'Á' | 'À' | 'Â' | 'Ã' => 'A',
            'é' | 'ê' | 'É' | 'Ê' => 'E',
            'í' | 'Í' => 'I',
            'ó' | 'ô' | 'õ' | 'Ó' | 'Ô' | 'Õ' => 'O',
            'ú' | 'ü' | 'Ú' => 'U',
            'ç' | 'Ç' => 'C',
            other => other.to_ascii_uppercase(),
        })
        .collect()
}

/// `text` cut or right-padded with spaces to exactly `width` characters, as
/// a `CHAR(width)` column returns it.
pub fn fixed_width(text: &str, width: usize) -> String {
    let mut out: String = text.chars().take(width).collect();
    let len = out.chars().count();
    out.extend(std::iter::repeat_n(' ', width - len));
    out
}

/// A date as the integer `YYYYMMDD`.
pub fn yyyymmdd((year, month, day): (u32, u32, u32)) -> i64 {
    i64::from(year) * 10_000 + i64::from(month) * 100 + i64::from(day)
}

pub fn random_date(rng: &mut Rng, first_year: u64, last_year: u64) -> (u32, u32, u32) {
    (
        rng.between(first_year, last_year) as u32,
        rng.between(1, 12) as u32,
        rng.between(1, 28) as u32,
    )
}

/// Days since 1899-12-30, the serial number a spreadsheet stores for a date.
pub fn spreadsheet_serial((year, month, day): (u32, u32, u32)) -> i64 {
    let (y, m) = if month <= 2 {
        (i64::from(year) - 1, i64::from(month) + 9)
    } else {
        (i64::from(year), i64::from(month) - 3)
    };
    let era = y.div_euclid(400);
    let year_of_era = y - era * 400;
    let day_of_year = (153 * m + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days_since_1970 = era * 146_097 + day_of_era - 719_468;
    days_since_1970 + 25_569
}

/// Cents as pt-BR prints money: `1.234,56`.
pub fn decimal_br(cents: u64) -> String {
    let units = (cents / 100).to_string();
    let mut grouped = String::new();
    for (i, digit) in units.chars().enumerate() {
        if i > 0 && (units.len() - i) % 3 == 0 {
            grouped.push('.');
        }
        grouped.push(digit);
    }
    format!("{grouped},{:02}", cents % 100)
}

/// An integer of ten or more digits the way a pt-BR spreadsheet shows a
/// number cell too narrow for it: six significant digits, `1,19877E+10`.
/// The digits past the sixth are gone for good.
pub fn scientific_br(digits: &str) -> String {
    let trimmed = digits.trim_start_matches('0');
    let exponent = trimmed.len().saturating_sub(1);
    let mut significant: Vec<u32> = trimmed.chars().filter_map(|c| c.to_digit(10)).collect();
    let round_up = significant.get(6).is_some_and(|d| *d >= 5);
    significant.truncate(6);
    let mut carry = round_up;
    for digit in significant.iter_mut().rev() {
        if !carry {
            break;
        }
        *digit += 1;
        carry = *digit == 10;
        if carry {
            *digit = 0;
        }
    }
    let (exponent, mut significant) = if carry {
        let mut lifted = vec![1];
        lifted.extend(significant.iter().take(5));
        (exponent + 1, lifted)
    } else {
        (exponent, significant)
    };
    while significant.len() > 1 && significant.last() == Some(&0) {
        significant.pop();
    }
    let mut text: String = significant
        .iter()
        .filter_map(|d| char::from_digit(*d, 10))
        .collect();
    if text.len() > 1 {
        text.insert(1, ',');
    }
    format!("{text}E+{exponent:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mojibake_reads_utf8_as_windows_1252() {
        assert_eq!(mojibake_cp1252("JOÃO"), "JOÃƒO");
        assert_eq!(mojibake_cp1252("Conceição"), "ConceiÃ§Ã£o");
        assert_eq!(mojibake_cp1252("ASSUNÇÃO"), "ASSUNÃ‡ÃƒO");
    }

    #[test]
    fn fixed_width_pads_and_cuts() {
        assert_eq!(fixed_width("ANA", 5), "ANA  ");
        assert_eq!(fixed_width("ANABELA", 5), "ANABE");
        assert_eq!(fixed_width("JOÃƒO", 6), "JOÃƒO ");
    }

    #[test]
    fn spreadsheet_serials_match_known_dates() {
        assert_eq!(spreadsheet_serial((1900, 3, 1)), 61);
        assert_eq!(spreadsheet_serial((1985, 3, 15)), 31_121);
        assert_eq!(spreadsheet_serial((2024, 1, 1)), 45_292);
    }

    #[test]
    fn money_and_long_numbers_print_the_pt_br_way() {
        assert_eq!(decimal_br(123_456), "1.234,56");
        assert_eq!(decimal_br(5), "0,05");
        assert_eq!(decimal_br(100_000_000), "1.000.000,00");
        assert_eq!(scientific_br("11987654321"), "1,19877E+10");
        assert_eq!(scientific_br("21999995000"), "2,2E+10");
        assert_eq!(scientific_br("1132654321"), "1,13265E+09");
        assert_eq!(scientific_br("4512345678"), "4,51235E+09");
    }

    #[test]
    fn dates_print_as_integers() {
        assert_eq!(yyyymmdd((1985, 3, 15)), 19_850_315);
    }
}
