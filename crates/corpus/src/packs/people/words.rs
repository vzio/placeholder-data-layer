//! The pack's own word lists. The shared lists in `generate/vocab.rs` are
//! frozen; everything this pack needs beyond them lives here.

/// CBO code, title, salary band in reais, and how often it is staffed.
pub const CARGOS: &[(&str, &str, u64, u64, u64)] = &[
    ("4110-10", "Assistente Administrativo", 2_200, 3_400, 60),
    ("4110-05", "Auxiliar de Escritório", 1_600, 2_400, 40),
    ("2524-05", "Analista de Recursos Humanos", 4_200, 7_800, 14),
    ("2124-05", "Analista de Sistemas", 6_500, 13_000, 18),
    ("3172-10", "Técnico de Suporte", 2_800, 4_600, 16),
    ("1421-05", "Gerente Administrativo", 11_000, 19_000, 5),
    ("1231-05", "Diretor Financeiro", 24_000, 42_000, 1),
    ("2522-10", "Contador", 6_000, 11_000, 6),
    ("4131-10", "Auxiliar de Contabilidade", 2_000, 3_200, 10),
    ("7842-05", "Auxiliar de Produção", 1_550, 2_300, 70),
    ("7250-10", "Montador de Máquinas", 2_300, 3_900, 30),
    ("9113-05", "Mecânico de Manutenção", 3_200, 5_400, 14),
    ("3911-15", "Supervisor de Produção", 5_500, 8_800, 8),
    ("4141-05", "Almoxarife", 2_100, 3_100, 9),
    ("7823-05", "Motorista de Caminhão", 2_900, 4_300, 12),
    ("5143-20", "Faxineiro", 1_450, 1_800, 20),
    ("5173-30", "Vigilante", 2_100, 2_900, 10),
    ("4221-05", "Recepcionista", 1_700, 2_500, 8),
    ("3541-25", "Assistente Comercial", 2_400, 3_800, 15),
    ("1423-05", "Gerente Comercial", 12_000, 21_000, 3),
];

pub const DEPARTAMENTOS: &[(&str, u64)] = &[
    ("Produção", 40),
    ("Administrativo", 18),
    ("Comercial", 10),
    ("Financeiro", 8),
    ("Logística", 10),
    ("Tecnologia", 8),
    ("Recursos Humanos", 4),
    ("Manutenção", 6),
];

/// Bank codes as payroll files carry them; codes only, no bank names.
pub const BANK_CODES: &[(&str, u64)] = &[
    ("001", 26),
    ("104", 22),
    ("237", 16),
    ("341", 18),
    ("033", 9),
    ("260", 6),
    ("077", 2),
    ("336", 1),
];

pub const PARENTESCOS: &[(&str, u64)] = &[
    ("Filho(a)", 62),
    ("Cônjuge", 28),
    ("Enteado(a)", 5),
    ("Pai/Mãe", 4),
    ("Menor sob guarda", 1),
];

/// CID-10 code and the Zipf-like share of consultations it receives.
pub const CID10: &[(&str, u64)] = &[
    ("Z00.0", 180),
    ("J06.9", 120),
    ("I10", 110),
    ("E11.9", 70),
    ("M54.5", 65),
    ("F41.1", 45),
    ("J45.9", 35),
    ("K29.7", 30),
    ("N39.0", 28),
    ("F32.9", 26),
    ("E78.5", 24),
    ("R51", 22),
    ("A09", 20),
    ("J02.9", 18),
    ("M79.1", 14),
    ("K21.9", 12),
    ("L30.9", 10),
    ("H10.9", 9),
    ("B34.9", 8),
    ("G43.9", 7),
    ("O26.9", 6),
    ("B24", 3),
    ("C50.9", 2),
    ("F20.9", 2),
];

pub const ESPECIALIDADES: &[(&str, u64)] = &[
    ("Clínica Geral", 40),
    ("Cardiologia", 12),
    ("Pediatria", 14),
    ("Ginecologia", 10),
    ("Ortopedia", 9),
    ("Dermatologia", 7),
    ("Psiquiatria", 5),
    ("Endocrinologia", 3),
];

pub const TIPOS_SANGUINEOS: &[(&str, u64)] = &[
    ("O+", 36),
    ("A+", 34),
    ("B+", 8),
    ("AB+", 2),
    ("O-", 9),
    ("A-", 8),
    ("B-", 2),
    ("AB-", 1),
];

pub const CONVENIOS: &[(&str, u64)] = &[
    ("Particular", 30),
    ("SUS", 25),
    ("Plano Vida Plena", 18),
    ("Saúde Integral Coop", 15),
    ("Bem Estar Seguros", 12),
];

pub const COMPLAINTS: &[&str] = &[
    "cefaleia há 3 dias",
    "dor lombar após esforço",
    "tosse seca e febre baixa",
    "ansiedade e insônia nas últimas semanas",
    "dor epigástrica pós-prandial",
    "ardência ao urinar",
    "falta de ar aos esforços",
    "retorno para avaliar exames",
    "check-up anual",
    "lesões pruriginosas nos braços",
];

pub const CONDUCTS: &[&str] = &[
    "Prescrito analgésico e repouso.",
    "Solicitados hemograma e glicemia de jejum.",
    "Orientada dieta e atividade física.",
    "Encaminhado à especialidade.",
    "Mantida medicação de uso contínuo.",
    "Retorno em 30 dias.",
];

/// Sensitive facts a clinical note discloses, as LGPD art. 5 II defines
/// sensitive personal data: health, sexual life, religion.
pub const SENSITIVE_FINDINGS: &[&str] = &[
    "Sorologia HIV reagente, em TARV.",
    "Gestante de 12 semanas.",
    "Histórico de tentativa de suicídio em 2019.",
    "Etilista, 10 doses por semana.",
    "Testemunha de Jeová, recusa transfusão.",
    "Em acompanhamento por dependência química.",
];

pub const TIPOS_CONTA: &[(&str, u64)] = &[("corrente", 62), ("poupanca", 25), ("pagamento", 13)];

pub const TRANSACTION_KINDS: &[(&str, u64)] = &[
    ("pix_enviado", 34),
    ("pix_recebido", 28),
    ("debito", 14),
    ("boleto", 9),
    ("ted", 4),
    ("tarifa", 6),
    ("saque", 5),
];

pub const MERCHANTS: &[&str] = &[
    "SUPERMERCADO BOA COMPRA",
    "POSTO ESTRELA DO SUL",
    "FARMACIA SAUDE MAIS",
    "PADARIA PAO DOURADO",
    "LOJAS HORIZONTE",
    "RESTAURANTE SABOR CASEIRO",
    "ACADEMIA CORPO ATIVO",
];

pub const SERIES: &[&str] = &[
    "1º ano EF",
    "2º ano EF",
    "3º ano EF",
    "4º ano EF",
    "5º ano EF",
    "6º ano EF",
    "7º ano EF",
    "8º ano EF",
    "9º ano EF",
    "1ª série EM",
    "2ª série EM",
    "3ª série EM",
];

pub const DISCIPLINAS: &[&str] = &[
    "Língua Portuguesa",
    "Matemática",
    "Ciências",
    "História",
    "Geografia",
    "Inglês",
    "Educação Física",
    "Arte",
];

pub const PARENTESCO_RESPONSAVEL: &[(&str, u64)] = &[
    ("Mãe", 58),
    ("Pai", 30),
    ("Avó", 6),
    ("Tio(a)", 3),
    ("Responsável legal", 3),
];

/// Body type and engine of a vehicle model, never a maker's trade name.
pub const VEHICLE_MODELS: &[(&str, u64)] = &[
    ("Hatch compacto 1.0 Flex", 30),
    ("Hatch 1.6 Flex", 14),
    ("Sedan médio 2.0 Flex", 12),
    ("SUV compacto 1.3 Turbo", 16),
    ("Picape média 2.8 Diesel", 7),
    ("Picape compacta 1.4 Flex", 8),
    ("Utilitário 1.6 Flex", 5),
    ("Motocicleta 160cc", 8),
];

pub const COVERAGES: &[(&str, u64)] = &[
    ("Compreensiva", 60),
    ("Roubo e furto", 18),
    ("Terceiros", 15),
    ("Perda total", 7),
];

pub const CLAIM_KINDS: &[(&str, u64)] = &[
    ("colisao", 48),
    ("roubo", 14),
    ("furto", 8),
    ("vidros", 18),
    ("alagamento", 4),
    ("incendio", 1),
    ("terceiros", 7),
];

pub const CLAIM_STATUS: &[(&str, u64)] = &[
    ("pago", 55),
    ("em_analise", 20),
    ("negado", 10),
    ("aguardando_documentos", 15),
];

pub const VEHICLE_COLOURS: &[(&str, u64)] = &[
    ("Branco", 30),
    ("Prata", 22),
    ("Preto", 22),
    ("Cinza", 14),
    ("Vermelho", 7),
    ("Azul", 5),
];

pub const VISIT_REASONS: &[&str] = &[
    "visita",
    "entrega de encomenda",
    "manutenção do ar-condicionado",
    "diarista",
    "técnico da internet",
    "mudança",
    "entrega de aplicativo",
];
