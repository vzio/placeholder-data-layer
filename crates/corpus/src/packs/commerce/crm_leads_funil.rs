//! A CRM lead funnel: sparse leads captured by forms, ads and events, with
//! UTM attribution, consent flags, corporate and free-provider emails, and
//! people who signed up more than once; and the sales team's interactions
//! with them.

use anonymizer::Value;
use serde::Deserialize;
use server::http::dto::{DatasetDto, WireType};

use super::{day, pick, timestamp, weighted};
use crate::generate::mess::date_iso;
use crate::generate::person::{Person, ascii_fold, phone_masked};
use crate::generate::rng::Rng;
use crate::generate::table::{TableBuilder, maybe_text, text};
use crate::generate::vocab::{COMPANY_SECTORS, COMPANY_STEMS, JOB_TITLES};
use crate::generate::{GenError, params};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LeadsParams {
    leads: usize,
    duplicados: usize,
    interacoes_max_por_lead: u64,
}

const SOURCES: &[(&str, u64)] = &[
    ("organico", 480),
    ("google_ads", 220),
    ("meta_ads", 130),
    ("indicacao", 70),
    ("linkedin", 50),
    ("evento", 30),
    ("parceiro", 20),
];

const STAGES: &[(&str, u64)] = &[
    ("lead", 520),
    ("mql", 210),
    ("sql", 110),
    ("oportunidade", 60),
    ("cliente", 40),
    ("perdido", 60),
];

const SEARCH_CAMPAIGNS: &[&str] = &[
    "bf2024_search_marca",
    "always_on_generico",
    "erp_pme_concorrentes",
    "remarketing_30d",
];

const SOCIAL_CAMPAIGNS: &[&str] = &[
    "lancamento_v3_video",
    "ebook_gestao_financeira",
    "webinar_fiscal_2024",
];

const EVENTS: &[&str] = &["feira_varejo_sp_2024", "summit_pme_poa", "meetup_bh_jun"];

const PARTNERS: &[&str] = &["contabil_parceira", "revenda_sul", "consultoria_norte"];

const INTERACTION_KINDS: &[(&str, u64)] = &[
    ("email", 45),
    ("ligacao", 25),
    ("whatsapp", 20),
    ("reuniao", 10),
];

const PLAIN_SUMMARIES: &[&str] = &[
    "Enviado material comercial.",
    "Sem resposta, tentar novamente semana que vem.",
    "Pediu proposta para 10 usuários.",
    "Já usa concorrente, contrato vence em março.",
    "Agendada demonstração.",
    "Achou caro, oferecer plano anual.",
];

/// The attribution a lead from `source` carries: organic and referral
/// traffic has no campaign, paid traffic has all three fields.
fn utm(rng: &mut Rng, source: &str) -> [Option<&'static str>; 3] {
    match source {
        "google_ads" => [
            Some("google"),
            Some("cpc"),
            Some(pick(rng, SEARCH_CAMPAIGNS)),
        ],
        "meta_ads" => [
            Some(if rng.chance(550) {
                "instagram"
            } else {
                "facebook"
            }),
            Some("paid_social"),
            Some(pick(rng, SOCIAL_CAMPAIGNS)),
        ],
        "linkedin" => [
            Some("linkedin"),
            Some(if rng.chance(600) {
                "paid_social"
            } else {
                "social"
            }),
            rng.chance(600).then(|| pick(rng, SOCIAL_CAMPAIGNS)),
        ],
        "evento" => [Some("evento"), Some("offline"), Some(pick(rng, EVENTS))],
        "indicacao" => [Some("indicacao"), Some("referral"), None],
        "parceiro" => [Some(pick(rng, PARTNERS)), Some("parceiro"), None],
        _ => [None, None, None],
    }
}

struct Lead {
    person: Person,
    nome: Option<String>,
    email: Option<String>,
    telefone: Option<String>,
    empresa: Option<String>,
    cargo: Option<&'static str>,
}

fn lead(rng: &mut Rng) -> Lead {
    let person = Person::generate(rng);
    let empresa = rng.chance(520).then(|| {
        format!(
            "{} {}",
            pick(rng, COMPANY_STEMS),
            pick(rng, COMPANY_SECTORS)
        )
    });
    let email = match (&empresa, rng.below(100)) {
        (_, 0..=17) => None,
        (Some(company), 18..=74) => {
            let first = ascii_fold(&person.first_name).to_lowercase();
            let last = person
                .full_name
                .rsplit(' ')
                .next()
                .map(|s| ascii_fold(s).to_lowercase())
                .unwrap_or_default();
            let domain = ascii_fold(company).to_lowercase();
            Some(format!("{first}.{last}@{domain}.com.br"))
        }
        _ => Some(person.email.clone()),
    };
    let nome = match rng.below(100) {
        0..=11 => None,
        12..=29 => Some(person.first_name.clone()),
        _ => Some(person.full_name.clone()),
    };
    let telefone = rng.chance(560).then(|| match rng.below(3) {
        0 => person.phone.clone(),
        _ => phone_masked(&person.phone),
    });
    let cargo = (empresa.is_some() && rng.chance(600))
        .then(|| rng.weighted(JOB_TITLES).copied().unwrap_or("Comprador"));
    Lead {
        person,
        nome,
        email,
        telefone,
        empresa,
        cargo,
    }
}

/// The same person filling in a form again weeks later: the email typed in
/// another case, the phone sometimes left out.
fn signs_up_again(rng: &mut Rng, original: &Lead) -> Lead {
    let person = Person {
        first_name: original.person.first_name.clone(),
        full_name: original.person.full_name.clone(),
        cpf: original.person.cpf.clone(),
        email: original.person.email.clone(),
        phone: original.person.phone.clone(),
        city: original.person.city,
        bairro: original.person.bairro,
        street: original.person.street.clone(),
        cep: original.person.cep.clone(),
        birth: original.person.birth,
    };
    let email = original.email.as_ref().map(|e| {
        if rng.chance(500) {
            e.to_uppercase()
        } else {
            e.clone()
        }
    });
    Lead {
        nome: Some(person.full_name.clone()),
        email,
        telefone: original.telefone.clone().filter(|_| rng.chance(500)),
        empresa: original.empresa.clone(),
        cargo: original.cargo,
        person,
    }
}

pub fn generate(seed: u64, raw: &serde_json::Value) -> Result<DatasetDto, GenError> {
    let p: LeadsParams = params(raw)?;
    if p.duplicados >= p.leads {
        return Err(GenError("duplicates need more leads".to_owned()));
    }
    let root = Rng::new(seed);
    let mut rng = root.fork("leads");
    let mut leads: Vec<Lead> = (0..p.leads).map(|_| lead(&mut rng)).collect();

    let mut duplicate_rng = root.fork("duplicados");
    let mut duplicated = 0;
    let mut attempts = 0;
    while duplicated < p.duplicados {
        attempts += 1;
        if attempts > 100 * p.leads {
            return Err(GenError(
                "too few leads with an email to duplicate".to_owned(),
            ));
        }
        let original = &leads[duplicate_rng.index(p.leads)];
        if original.email.is_none() || original.nome.is_none() {
            continue;
        }
        let again = signs_up_again(&mut duplicate_rng, original);
        let at = duplicate_rng.index(leads.len() + 1);
        leads.insert(at, again);
        duplicated += 1;
    }

    let mut table = TableBuilder::new("leads")
        .primary_key("id", WireType::Integer)
        .column("nome", WireType::String)
        .column("email", WireType::String)
        .column("telefone", WireType::String)
        .column("empresa", WireType::String)
        .column("cargo", WireType::String)
        .column("origem", WireType::String)
        .column("utm_source", WireType::String)
        .column("utm_medium", WireType::String)
        .column("utm_campaign", WireType::String)
        .column("consentimento_marketing", WireType::Boolean)
        .column("data_consentimento", WireType::String)
        .column("etapa_funil", WireType::String)
        .column("criado_em", WireType::String);
    let mut interacoes = TableBuilder::new("interacoes")
        .primary_key("id", WireType::Integer)
        .foreign_key("lead_id", WireType::Integer, "leads", "id")
        .column("tipo", WireType::String)
        .column("data", WireType::String)
        .column("resumo", WireType::String);

    let mut row_rng = root.fork("leads-linhas");
    let mut interaction_rng = root.fork("interacoes");
    for (index, lead) in leads.iter().enumerate() {
        let id = index as i64 + 1;
        let source = weighted(&mut row_rng, SOURCES);
        let [utm_source, utm_medium, utm_campaign] = utm(&mut row_rng, source);
        let created = day(&mut row_rng, 2023, 2024);
        let consent = match row_rng.below(100) {
            0..=7 => None,
            8..=61 => Some(true),
            _ => Some(false),
        };
        let consent_date = (consent == Some(true)).then(|| date_iso(created));
        let stage = weighted(&mut row_rng, STAGES);
        table.push(vec![
            Value::Int(id),
            maybe_text(lead.nome.clone()),
            maybe_text(lead.email.clone()),
            maybe_text(lead.telefone.clone()),
            maybe_text(lead.empresa.clone()),
            maybe_text(lead.cargo.map(str::to_owned)),
            text(source),
            maybe_text(utm_source.map(str::to_owned)),
            maybe_text(utm_medium.map(str::to_owned)),
            maybe_text(utm_campaign.map(str::to_owned)),
            consent.map_or(Value::Null, Value::Bool),
            maybe_text(consent_date),
            text(stage),
            text(timestamp(&mut row_rng, created)),
        ])?;

        let touches = match stage {
            "lead" => interaction_rng.below(2),
            _ => interaction_rng.between(1, p.interacoes_max_por_lead),
        };
        for _ in 0..touches {
            let kind = weighted(&mut interaction_rng, INTERACTION_KINDS);
            let resumo = match interaction_rng.below(100) {
                0..=39 => None,
                40..=87 => Some(pick(&mut interaction_rng, PLAIN_SUMMARIES).to_owned()),
                88..=94 => Some(format!(
                    "Pediu para ligar no {} depois das 14h.",
                    phone_masked(&lead.person.phone)
                )),
                _ => Some(format!(
                    "Falei com {}, vai levar para a diretoria.",
                    lead.person.full_name
                )),
            };
            interacoes.push(vec![
                Value::Int(interacoes.len() as i64 + 1),
                Value::Int(id),
                text(kind),
                text(date_iso(day(&mut interaction_rng, 2023, 2024))),
                maybe_text(resumo),
            ])?;
        }
    }

    Ok(DatasetDto {
        tables: vec![table.build(), interacoes.build()],
    })
}
