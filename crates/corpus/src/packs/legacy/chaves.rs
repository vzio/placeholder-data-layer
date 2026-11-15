//! An HR schema keyed by CPF text: the person's CPF is the primary key and
//! the foreign key of every table around it, managers point back into the
//! same table, departments and people reference each other, branches and
//! assignments carry composite primary keys, and a time-clock table joins
//! on the CPF's digits without declaring it. Ex-employees and closed
//! branches leave orphans behind.

use anonymizer::Value;
use serde::Deserialize;
use server::http::dto::{DatasetDto, TableDto, WireType};

use super::legacy_text::random_date;
use crate::generate::docs::{Document, text_of};
use crate::generate::mess::date_iso;
use crate::generate::person::Person;
use crate::generate::rng::Rng;
use crate::generate::table::{TableBuilder, maybe_text, text};
use crate::generate::{GenError, params};

const DEPARTMENTS: &[&str] = &[
    "Diretoria",
    "Financeiro",
    "Contabilidade",
    "Recursos Humanos",
    "Comercial",
    "Logística",
    "Compras",
    "Tecnologia",
    "Jurídico",
    "Atendimento",
    "Produção",
    "Qualidade",
];

const BRANCH_KINDS: &[&str] = &["Matriz", "Loja", "Centro de Distribuição", "Escritório"];

const KINSHIP: &[(&str, u64)] = &[
    ("filho(a)", 60),
    ("cônjuge", 30),
    ("enteado(a)", 7),
    ("pai/mãe", 3),
];

const ROLES: &[&str] = &[
    "Analista",
    "Assistente",
    "Coordenador(a)",
    "Vendedor(a)",
    "Operador(a)",
];

/// The companies of the group and how many branches each has.
const BRANCHES_PER_COMPANY: &[i64] = &[7, 5, 4];

/// A branch code no branch carries any more.
const CLOSED_BRANCH: i64 = 99;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChavesParams {
    pessoas: usize,
    gestores_desligados: usize,
    lotacoes_filial_fechada: usize,
    lotacoes_par_inexistente: usize,
    dependentes: usize,
    dependentes_orfaos: usize,
    batidas_ponto: usize,
}

/// One employee's posting to a branch from a start date.
#[derive(Clone, Copy)]
struct Assignment {
    person: usize,
    company: i64,
    branch: i64,
    start: (u32, u32, u32),
}

/// Marks `column` of `table` as a member of its composite primary key.
fn into_primary_key(table: &mut TableDto, column: &str) -> Result<(), GenError> {
    let spec = table
        .columns
        .iter_mut()
        .find(|c| c.name == column)
        .ok_or_else(|| GenError(format!("no column `{column}` in `{}`", table.name)))?;
    spec.pk = true;
    spec.nullable = false;
    Ok(())
}

fn cpf_text(cpf: &[u8]) -> Value {
    text(Document::Cpf.masked(cpf))
}

pub fn chaves_compostas_e_ciclos(
    seed: u64,
    raw: &serde_json::Value,
) -> Result<DatasetDto, GenError> {
    let p: ChavesParams = params(raw)?;
    if p.pessoas < DEPARTMENTS.len() * 2 {
        return Err(GenError(
            "too few people to staff every department".to_owned(),
        ));
    }
    let root = Rng::new(seed);
    let mut rng = root.fork("pessoa");
    let people: Vec<Person> = (0..p.pessoas).map(|_| Person::generate(&mut rng)).collect();
    let former: Vec<Vec<u8>> = (0..p.gestores_desligados.max(p.dependentes_orfaos))
        .map(|_| Document::Cpf.generate(&mut rng))
        .collect();

    // People are ordered by seniority: the first of each department heads
    // it, and everyone else reports to someone earlier in the list.
    let mut pessoa = TableBuilder::new("pessoa")
        .primary_key("cpf", WireType::String)
        .column("nome", WireType::String)
        .column("email", WireType::String)
        .foreign_key("departamento_id", WireType::Integer, "departamento", "id")
        .foreign_key("gestor_cpf", WireType::String, "pessoa", "cpf");
    let departments = DEPARTMENTS.len();
    let mut former_managed = 0;
    for (index, person) in people.iter().enumerate() {
        let department = if index < departments {
            index
        } else {
            rng.index(departments)
        };
        let manager = if index == 0 {
            Value::Null
        } else if index == 1 {
            // Two people who name each other as manager: a cycle in the data.
            cpf_text(&people[2].cpf)
        } else if index == 2 {
            cpf_text(&people[1].cpf)
        } else if index < departments {
            cpf_text(&people[0].cpf)
        } else if former_managed < p.gestores_desligados && rng.chance(40) {
            former_managed += 1;
            cpf_text(&former[former_managed - 1])
        } else {
            cpf_text(&people[rng.index(index)].cpf)
        };
        pessoa.push(vec![
            cpf_text(&person.cpf),
            text(person.full_name.clone()),
            text(person.email.clone()),
            Value::Int(department as i64 + 1),
            manager,
        ])?;
    }
    if former_managed < p.gestores_desligados {
        return Err(GenError(
            "too few people to report to former managers".to_owned(),
        ));
    }

    let mut departamento = TableBuilder::new("departamento")
        .primary_key("id", WireType::Integer)
        .column("nome", WireType::String)
        .foreign_key("responsavel_cpf", WireType::String, "pessoa", "cpf");
    for (index, name) in DEPARTMENTS.iter().enumerate() {
        departamento.push(vec![
            Value::Int(index as i64 + 1),
            text(*name),
            cpf_text(&people[index].cpf),
        ])?;
    }

    let mut rng = root.fork("filial");
    let mut filial = TableBuilder::new("filial")
        .primary_key("cod_empresa", WireType::Integer)
        .column("cod_filial", WireType::Integer)
        .column("nome_filial", WireType::String)
        .column("cidade", WireType::String)
        .foreign_key("gerente_cpf", WireType::String, "pessoa", "cpf");
    let mut branches = Vec::new();
    for (company, count) in BRANCHES_PER_COMPANY.iter().enumerate() {
        for branch in 1..=*count {
            let city = crate::generate::person::weighted_city(&mut rng);
            let kind = if branch == 1 {
                "Matriz"
            } else {
                rng.pick(&BRANCH_KINDS[1..]).copied().unwrap_or("Loja")
            };
            let manager = &people[rng.index(p.pessoas)];
            branches.push((company as i64 + 1, branch));
            filial.push(vec![
                Value::Int(company as i64 + 1),
                Value::Int(branch),
                text(format!("{kind} {}", city.name)),
                text(city.name),
                cpf_text(&manager.cpf),
            ])?;
        }
    }
    let mut filial = filial.build();
    into_primary_key(&mut filial, "cod_filial")?;

    let mut rng = root.fork("lotacao");
    let mut lotacao = TableBuilder::new("lotacao")
        .foreign_key("cpf", WireType::String, "pessoa", "cpf")
        .foreign_key("cod_empresa", WireType::Integer, "filial", "cod_empresa")
        .foreign_key("cod_filial", WireType::Integer, "filial", "cod_filial")
        .column("data_inicio", WireType::String)
        .column("cargo", WireType::String);
    let mut assignments: Vec<Assignment> = Vec::new();
    for person in 0..p.pessoas {
        let mut start = random_date(&mut rng, 2008, 2016);
        for _ in 0..rng.between(1, 3) {
            let (company, branch) = branches[rng.index(branches.len())];
            assignments.push(Assignment {
                person,
                company,
                branch,
                start,
            });
            start.0 += rng.between(1, 3) as u32;
        }
    }
    let mut closed = 0;
    let mut impossible_pairs = 0;
    while closed < p.lotacoes_filial_fechada || impossible_pairs < p.lotacoes_par_inexistente {
        let row = rng.index(assignments.len());
        let Assignment {
            company, branch, ..
        } = assignments[row];
        if branch == CLOSED_BRANCH || branch > BRANCHES_PER_COMPANY[2] {
            continue;
        }
        if closed < p.lotacoes_filial_fechada {
            assignments[row].branch = CLOSED_BRANCH;
            closed += 1;
        } else if company == 3 {
            // Both codes exist on their own; the pair names no branch.
            assignments[row].branch = BRANCHES_PER_COMPANY[0];
            impossible_pairs += 1;
        }
    }
    for assignment in &assignments {
        lotacao.push(vec![
            cpf_text(&people[assignment.person].cpf),
            Value::Int(assignment.company),
            Value::Int(assignment.branch),
            text(date_iso(assignment.start)),
            text(rng.pick(ROLES).copied().unwrap_or("Analista")),
        ])?;
    }
    let mut lotacao = lotacao.build();
    for column in ["cpf", "cod_empresa", "cod_filial", "data_inicio"] {
        into_primary_key(&mut lotacao, column)?;
    }

    let mut rng = root.fork("dependente");
    let mut dependente = TableBuilder::new("dependente")
        .primary_key("cpf_dependente", WireType::String)
        .foreign_key("cpf_titular", WireType::String, "pessoa", "cpf")
        .column("nome", WireType::String)
        .column("parentesco", WireType::String)
        .column("data_nascimento", WireType::String);
    let mut orphaned = 0;
    for _ in 0..p.dependentes {
        let child = Person::generate(&mut rng);
        let holder = if orphaned < p.dependentes_orfaos && rng.chance(30) {
            orphaned += 1;
            &former[orphaned - 1]
        } else {
            &people[rng.index(p.pessoas)].cpf
        };
        dependente.push(vec![
            cpf_text(&child.cpf),
            cpf_text(holder),
            text(child.full_name),
            text(rng.weighted(KINSHIP).copied().unwrap_or("filho(a)")),
            text(date_iso(child.birth)),
        ])?;
    }
    if orphaned < p.dependentes_orfaos {
        return Err(GenError("too few dependents to orphan".to_owned()));
    }

    let mut rng = root.fork("ponto");
    let mut ponto = TableBuilder::new("ponto_eletronico")
        .primary_key("id", WireType::Integer)
        .column("cpf_funcionario", WireType::String)
        .column("data", WireType::String)
        .column("entrada", WireType::String)
        .column("saida", WireType::String);
    for row in 0..p.batidas_ponto {
        let cpf = if rng.chance(15) {
            former.get(rng.index(former.len())).cloned()
        } else {
            Some(people[rng.index(p.pessoas)].cpf.clone())
        };
        let entry = rng.between(7 * 60, 10 * 60);
        let exit = entry + rng.between(8 * 60, 10 * 60);
        let leave = if rng.chance(30) {
            None
        } else {
            Some(format!("{:02}:{:02}", exit / 60, exit % 60))
        };
        ponto.push(vec![
            Value::Int(row as i64 + 1),
            maybe_text(cpf.map(|c| text_of(&c))),
            text(date_iso(random_date(&mut rng, 2023, 2024))),
            text(format!("{:02}:{:02}", entry / 60, entry % 60)),
            maybe_text(leave),
        ])?;
    }

    Ok(DatasetDto {
        tables: vec![
            pessoa.build(),
            departamento.build(),
            filial,
            lotacao,
            dependente.build(),
            ponto.build(),
        ],
    })
}
