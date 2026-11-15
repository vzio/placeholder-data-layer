//! Brazilian identifiers the shared generators do not cover, written from
//! their published rules: PIS/PASEP, CNS, RENAVAM, state RG numbers, bank
//! branch and account numbers, vehicle plates and chassis numbers.

use crate::generate::docs::text_of;
use crate::generate::rng::Rng;

const PIS_WEIGHTS: [u32; 10] = [3, 2, 9, 8, 7, 6, 5, 4, 3, 2];
const RENAVAM_WEIGHTS: [u32; 10] = [3, 2, 9, 8, 7, 6, 5, 4, 3, 2];

fn digits(rng: &mut Rng, count: usize) -> Vec<u8> {
    (0..count).map(|_| rng.below(10) as u8).collect()
}

fn weighted_sum(digits: &[u8], weights: &[u32]) -> u32 {
    digits
        .iter()
        .zip(weights)
        .map(|(d, w)| u32::from(*d) * w)
        .sum()
}

fn parse_digits(text: &str, width: usize) -> Option<Vec<u8>> {
    let digits: Vec<u8> = text
        .bytes()
        .filter(u8::is_ascii_digit)
        .map(|b| b - b'0')
        .collect();
    (digits.len() == width).then_some(digits)
}

/// The PIS/PASEP/NIT check digit of a ten-digit base.
pub fn pis_check_digit(base: &[u8]) -> u8 {
    let rest = weighted_sum(base, &PIS_WEIGHTS) % 11;
    if rest < 2 { 0 } else { (11 - rest) as u8 }
}

pub fn pis_is_valid(text: &str) -> bool {
    parse_digits(text, 11)
        .is_some_and(|d| d.iter().any(|x| *x != d[0]) && pis_check_digit(&d[..10]) == d[10])
}

/// A valid PIS/PASEP: the first digit is 1 (PIS) or 2 (PASEP), as issued.
pub fn pis_generate(rng: &mut Rng) -> Vec<u8> {
    let mut base = vec![if rng.chance(800) { 1 } else { 2 }];
    base.extend(digits(rng, 9));
    let check = pis_check_digit(&base);
    base.push(check);
    base
}

/// `XXX.XXXXX.XX-X`.
pub fn pis_masked(d: &[u8]) -> String {
    let t = text_of(d);
    format!("{}.{}.{}-{}", &t[0..3], &t[3..8], &t[8..10], &t[10..11])
}

/// A CNS is valid when its fifteen digits, weighted 15 down to 1, sum to a
/// multiple of eleven, and it starts with 1, 2, 7, 8 or 9.
pub fn cns_is_valid(text: &str) -> bool {
    let Some(d) = parse_digits(text, 15) else {
        return false;
    };
    let weights: Vec<u32> = (1..=15).rev().collect();
    matches!(d[0], 1 | 2 | 7 | 8 | 9) && weighted_sum(&d, &weights) % 11 == 0
}

/// A definitive CNS, derived from an eleven-digit PIS-like stem starting
/// with 1 or 2, or a provisional one starting with 7, 8 or 9.
pub fn cns_generate(rng: &mut Rng) -> Vec<u8> {
    if rng.chance(750) {
        let mut stem = vec![1 + rng.below(2) as u8];
        stem.extend(digits(rng, 10));
        let weights: Vec<u32> = (5..=15).rev().collect();
        let sum = weighted_sum(&stem, &weights);
        let mut check = 11 - sum % 11;
        let mut middle = [0, 0, 0];
        if check == 11 {
            check = 0;
        }
        if check == 10 {
            check = 11 - (sum + 2) % 11;
            middle = [0, 0, 1];
        }
        stem.extend(middle);
        stem.push(check as u8);
        return stem;
    }
    loop {
        let mut d = vec![7 + rng.below(3) as u8];
        d.extend(digits(rng, 13));
        let weights: Vec<u32> = (2..=15).rev().collect();
        let needed = (11 - weighted_sum(&d, &weights) % 11) % 11;
        if needed < 10 {
            d.push(needed as u8);
            return d;
        }
    }
}

/// `XXX XXXX XXXX XXXX`, as printed on the card.
pub fn cns_spaced(d: &[u8]) -> String {
    let t = text_of(d);
    format!("{} {} {} {}", &t[0..3], &t[3..7], &t[7..11], &t[11..15])
}

/// The RENAVAM check digit of a ten-digit base.
pub fn renavam_check_digit(base: &[u8]) -> u8 {
    let rest = (weighted_sum(base, &RENAVAM_WEIGHTS) * 10) % 11;
    if rest == 10 { 0 } else { rest as u8 }
}

/// A RENAVAM of up to eleven digits; a shorter all-digit value is one that
/// lost its leading zeros, or a nine-digit number from before the
/// eleven-digit format, both padded back.
pub fn renavam_is_valid(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.len() > 11 || !trimmed.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let padded = format!("{trimmed:0>11}");
    parse_digits(&padded, 11).is_some_and(|d| renavam_check_digit(&d[..10]) == d[10])
}

/// A valid RENAVAM; older registrations start with `00`.
pub fn renavam_generate(rng: &mut Rng) -> Vec<u8> {
    let mut base = if rng.chance(300) {
        let mut b = vec![0, 0];
        b.extend(digits(rng, 8));
        b
    } else {
        let mut b = vec![1 + rng.below(9) as u8];
        b.extend(digits(rng, 9));
        b
    };
    let check = renavam_check_digit(&base);
    base.push(check);
    base
}

/// The SP-style RG check character over eight digits: weights 2 to 9, the
/// remainder's complement, `X` for ten.
pub fn rg_sp_check(base: &[u8]) -> char {
    let weights: Vec<u32> = (2..=9).collect();
    match (11 - weighted_sum(base, &weights) % 11) % 11 {
        10 => 'X',
        n => char::from(b'0' + n as u8),
    }
}

/// An RG in the layout its issuing state uses: SP `12.345.678-X`, RJ
/// `12.345.678-9`, MG `MG-12.345.678`, and bare digits elsewhere.
pub fn rg_for_state(rng: &mut Rng, uf: &str) -> String {
    let base = digits(rng, 8);
    let t = text_of(&base);
    let dotted = format!("{}.{}.{}", &t[0..2], &t[2..5], &t[5..8]);
    match uf {
        "SP" => format!("{dotted}-{}", rg_sp_check(&base)),
        "RJ" => format!("{dotted}-{}", rng.below(10)),
        "MG" => format!("MG-{dotted}"),
        _ => format!("{t}{}", rng.below(10)),
    }
}

/// A mod-11 verifier, weights rising from 2 right to left, `X` for ten:
/// how many banks print the digit after a branch or account number.
pub fn bank_verifier(number: &[u8]) -> char {
    let sum: u32 = number
        .iter()
        .rev()
        .zip((2..=9).cycle())
        .map(|(d, w)| u32::from(*d) * w)
        .sum();
    match 11 - sum % 11 {
        10 => 'X',
        11 => '0',
        n => char::from(b'0' + n as u8),
    }
}

/// A four-digit branch and its verifier: `1234-5`.
pub fn bank_branch(rng: &mut Rng) -> String {
    let number = digits(rng, 4);
    format!("{}-{}", text_of(&number), bank_verifier(&number))
}

/// An account of five to eight digits and its verifier: `123456-7`.
pub fn bank_account(rng: &mut Rng) -> String {
    let width = 5 + rng.below(4) as usize;
    let mut number = digits(rng, width);
    if number[0] == 0 {
        number[0] = 1;
    }
    format!("{}-{}", text_of(&number), bank_verifier(&number))
}

const PLATE_LETTERS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ";

fn letters(rng: &mut Rng, count: usize) -> String {
    (0..count)
        .map(|_| char::from(PLATE_LETTERS[rng.index(PLATE_LETTERS.len())]))
        .collect()
}

/// A pre-Mercosul plate, `ABC1234`.
pub fn plate_old(rng: &mut Rng) -> String {
    format!("{}{:04}", letters(rng, 3), rng.below(10_000))
}

/// The pre-Mercosul layout with its hyphen, `ABC-1234`.
pub fn plate_old_hyphenated(plate: &str) -> String {
    let (letters, number) = plate.split_at(3.min(plate.len()));
    format!("{letters}-{number}")
}

/// A Mercosul plate, `ABC1D23`.
pub fn plate_mercosul(rng: &mut Rng) -> String {
    format!(
        "{}{}{}{:02}",
        letters(rng, 3),
        rng.below(10),
        letters(rng, 1),
        rng.below(100)
    )
}

/// The Mercosul conversion of an old plate: the second digit becomes the
/// letter at that position, `ABC1234` to `ABC1C34`.
pub fn plate_converted(old: &str) -> String {
    let bytes = old.as_bytes();
    if bytes.len() != 7 || !bytes[4].is_ascii_digit() {
        return old.to_owned();
    }
    let letter = char::from(b'A' + (bytes[4] - b'0'));
    format!("{}{letter}{}", &old[..4], &old[5..])
}

const VIN_CHARS: &[u8] = b"ABCDEFGHJKLMNPRSTUVWXYZ0123456789";
const VIN_WMIS: &[&str] = &[
    "9BW", "9BG", "9BD", "93H", "9BF", "936", "9C2", "94D", "93Y",
];
const VIN_YEARS: &[u8] = b"ABCDEFGHJKLMNPRS";

/// A seventeen-character chassis number with a Brazilian manufacturer
/// prefix and a model-year code, never `I`, `O` or `Q`.
pub fn chassis(rng: &mut Rng) -> String {
    let wmi = rng.pick(VIN_WMIS).copied().unwrap_or("9BW");
    let body: String = (0..5)
        .map(|_| char::from(VIN_CHARS[rng.index(VIN_CHARS.len())]))
        .collect();
    let check = char::from(VIN_CHARS[rng.index(VIN_CHARS.len())]);
    let year = char::from(VIN_YEARS[rng.index(VIN_YEARS.len())]);
    let plant = char::from(VIN_CHARS[rng.index(VIN_CHARS.len())]);
    format!("{wmi}{body}{check}{year}{plant}{:06}", rng.below(1_000_000))
}

/// A random-key PIX address: a lowercase version-4 UUID.
pub fn uuid_v4(rng: &mut Rng) -> String {
    let a = rng.next_u64();
    let b = rng.next_u64();
    let hex = format!("{a:016x}{b:016x}");
    let variant = ["8", "9", "a", "b"][rng.index(4)];
    format!(
        "{}-{}-4{}-{variant}{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[13..16],
        &hex[17..20],
        &hex[20..32]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn published_pis_examples_validate() {
        // 1*3+2*2+0*9+5*8+4*7+3*6+2*5+1*4+0*3+9*2 = 125; 125 % 11 = 4; 11 - 4 = 7.
        assert_eq!(pis_check_digit(&[1, 2, 0, 5, 4, 3, 2, 1, 0, 9]), 7);
        assert!(pis_is_valid("120.54321.09-7"));
        assert!(!pis_is_valid("120.54321.09-8"));
        assert!(!pis_is_valid("111.11111.11-1"));
    }

    #[test]
    fn generated_identifiers_validate() {
        let mut rng = Rng::new(17);
        for _ in 0..500 {
            assert!(pis_is_valid(&pis_masked(&pis_generate(&mut rng))));
            assert!(cns_is_valid(&cns_spaced(&cns_generate(&mut rng))));
            assert!(renavam_is_valid(&text_of(&renavam_generate(&mut rng))));
        }
    }

    #[test]
    fn a_hand_computed_cns_validates_and_a_changed_digit_does_not() {
        // Stem 12345678901 weighted 15 down to 5 sums to 440, a multiple of 11.
        assert!(cns_is_valid("123 4567 8901 0000"));
        assert!(!cns_is_valid("123 4567 8901 0001"));
        assert!(!cns_is_valid("323 4567 8901 0000"));
        let mut rng = Rng::new(3);
        let mut cns = cns_generate(&mut rng);
        cns[14] = (cns[14] + 1) % 10;
        assert!(!cns_is_valid(&text_of(&cns)));
    }

    #[test]
    fn renavam_check_digit_matches_a_hand_computation() {
        // 0*3+0*2+6*9+3*8+9*7+2*6+3*5+5*4+4*3+0*2 = 200; 2000 % 11 = 9.
        assert_eq!(renavam_check_digit(&[0, 0, 6, 3, 9, 2, 3, 5, 4, 0]), 9);
        assert!(renavam_is_valid("00639235409"));
        assert!(renavam_is_valid("639235409"));
        assert!(!renavam_is_valid("00639235408"));
    }

    #[test]
    fn plates_and_chassis_have_their_layouts() {
        let mut rng = Rng::new(5);
        let old = plate_old(&mut rng);
        assert_eq!(old.len(), 7);
        assert_eq!(plate_old_hyphenated("ABC1234"), "ABC-1234");
        assert_eq!(plate_converted("ABC1234"), "ABC1C34");
        assert_eq!(plate_mercosul(&mut rng).len(), 7);
        let vin = chassis(&mut rng);
        assert_eq!(vin.len(), 17);
        assert!(!vin.contains(['I', 'O', 'Q']));
        assert_eq!(uuid_v4(&mut rng).len(), 36);
    }

    #[test]
    fn rg_layouts_follow_the_state() {
        let mut rng = Rng::new(8);
        assert!(rg_for_state(&mut rng, "MG").starts_with("MG-"));
        assert_eq!(rg_for_state(&mut rng, "SP").len(), 12);
        assert_eq!(rg_for_state(&mut rng, "BA").len(), 9);
        assert_eq!(bank_verifier(&[0, 0, 0, 1]), '9');
    }
}
