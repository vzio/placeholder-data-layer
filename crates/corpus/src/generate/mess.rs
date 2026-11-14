//! The ways real production data is dirty, as small reusable distortions.

use super::rng::Rng;

pub const PLACEHOLDER_NAME: &str = "Fulano de Tal";
pub const PLACEHOLDER_EMAIL: &str = "teste@teste.com";
pub const PLACEHOLDER_CPF: &str = "111.444.777-35";
pub const PLACEHOLDER_PHONE: &str = "(00) 00000-0000";

pub const SENTINELS: &[&str] = &["N/A", "-", "NAO TEM", "nao informado"];

/// Upper case, lower case, or left alone.
pub fn case_mess(rng: &mut Rng, text: &str) -> String {
    match rng.below(3) {
        0 => text.to_uppercase(),
        1 => text.to_lowercase(),
        _ => text.to_owned(),
    }
}

pub fn pad(rng: &mut Rng, text: &str) -> String {
    match rng.below(3) {
        0 => format!(" {text}"),
        1 => format!("{text} "),
        _ => format!("  {text}  "),
    }
}

/// UTF-8 bytes read back as Latin-1: `João` becomes `JoÃ£o`.
pub fn mojibake(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 2);
    for c in text.chars() {
        if c.is_ascii() {
            out.push(c);
        } else {
            let mut buffer = [0u8; 4];
            for byte in c.encode_utf8(&mut buffer).bytes() {
                out.push(char::from(byte));
            }
        }
    }
    out
}

/// A realistic misspelling of a provider domain; an unknown domain keeps
/// its spelling and loses its last letter.
pub fn domain_typo(rng: &mut Rng, email: &str) -> String {
    let Some((local, domain)) = email.rsplit_once('@') else {
        return email.to_owned();
    };
    let typos: &[&str] = match domain {
        "gmail.com" => &["gmial.com", "gmail.con", "gmai.com", "gmail.com.br"],
        "hotmail.com" => &["hotmail.con", "hotmal.com", "hotmail.com.br"],
        "outlook.com" => &["outlok.com", "outlook.con"],
        "yahoo.com.br" => &["yaho.com.br", "yahoo.com.bt"],
        _ => &[],
    };
    match rng.pick(typos) {
        Some(typo) => format!("{local}@{typo}"),
        None => {
            let mut shortened = domain.to_owned();
            shortened.pop();
            format!("{local}@{shortened}")
        }
    }
}

pub fn without_at_sign(email: &str) -> String {
    email.replacen('@', "", 1)
}

/// A free-text note that repeats the row's own CPF or phone, the way an
/// operator types them into an observations field.
pub fn note_embedding(rng: &mut Rng, cpf: &str, phone: &str) -> String {
    match rng.below(5) {
        0 => format!("Cliente pediu para ligar no {phone} antes da entrega"),
        1 => format!("CPF {cpf} informado no balcão"),
        2 => format!("Contato alternativo: {phone}. Confirmar CPF {cpf}"),
        3 => format!("cpf: {cpf} - reenviar nota"),
        _ => format!("whats {phone}"),
    }
}

pub const PLAIN_NOTES: &[&str] = &[
    "Entregar após as 18h",
    "Prefere contato por e-mail",
    "Cliente antigo, desde a loja física",
    "Portão azul, tocar a campainha",
    "Não deixar com o porteiro",
    "Pagamento sempre no pix",
];

pub fn date_iso((year, month, day): (u32, u32, u32)) -> String {
    format!("{year:04}-{month:02}-{day:02}")
}

pub fn date_br((year, month, day): (u32, u32, u32)) -> String {
    format!("{day:02}/{month:02}/{year:04}")
}

pub const IMPOSSIBLE_DATES: &[&str] = &[
    "2023-02-30",
    "31/04/2022",
    "2022-13-01",
    "00/00/0000",
    "2021-06-31",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mojibake_reads_utf8_as_latin1() {
        assert_eq!(mojibake("João"), "JoÃ£o");
        assert_eq!(mojibake("Conceição"), "ConceiÃ§Ã£o");
    }

    #[test]
    fn a_domain_typo_keeps_the_local_part() {
        let mut rng = Rng::new(1);
        let typo = domain_typo(&mut rng, "ana.silva@gmail.com");
        assert!(typo.starts_with("ana.silva@"));
        assert_ne!(typo, "ana.silva@gmail.com");
    }
}
