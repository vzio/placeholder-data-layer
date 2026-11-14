# colunas_mentirosas

One table, `cadastro`, of sixty clean, synthetic rows whose column names lie about every value —
the shape of a legacy schema where columns were repurposed instead of renamed.

| Column | Holds |
|---|---|
| `telefone` | email addresses |
| `email` | mobile phones, masked |
| `cpf` | valid CNPJs, masked |
| `nome` | city names, a coarse location rather than a person |
| `observacao` | full names |
| `codigo_cliente` | valid CPFs as bare digits |
| `data_cadastro` | stock free-text phrases, not dates |

The values carry no mess on purpose, so a miss can only come from trusting the column name over
the values. Every direct identifier must be found where it really is and anonymized as what it
is; what is not personal (the cities, the stock phrases) is wanted kept, which is an open ruling.

The dataset is curated and committed as `dataset.json`.
