# adversarial_quase_pii

A home-textiles retailer's `produtos` (40), `lojas` (24) and `movimentos` (120) in which almost
nothing is personal and much looks it. The dataset is curated and committed as `dataset.json`.

| Column | Looks like | Is |
|---|---|---|
| `produtos.sku` | a masked CEP, with real CEP prefixes (`04512-246`) | a catalogue key |
| `produtos.codigo_interno`, `movimentos.protocolo` | an unmasked CPF or a phone (eleven digits; two protocols start with `00`) | internal numbers, every one failing the CPF check digits |
| `lojas.codigo_fiscal` | an unmasked CNPJ (fourteen digits) | a fiscal code failing the CNPJ check digits |
| `produtos.cor`, `produtos.linha` | given names: Rosa, Celeste, Linha Vitória, Coleção Santa Cruz | colours and product lines |
| `lojas.nome_loja`, `lojas.bairro` | names: Casa Rosa, Branco & Cia Enxovais, Santa Cruz | businesses and neighbourhoods |
| `lojas.cidade` | people: Getúlio Vargas, Teófilo Otoni, Santos Dumont, Presidente Prudente | cities |
| `lojas.telefone_sac` | a phone | a company's 0800 line |
| `produtos.data_lancamento` | a birth date | a launch date |
| `lojas.gerente` | a full name | **a full name**: the only personal column, and the control |

Over-redaction cannot be measured by what disappears, so it is measured by what must not: every
column but the managers' is wanted byte for byte, and a non-personal column that the engine
redacts or resamples fails its expectation. The control (every manager's name changes) keeps the
scenario from being passed by an engine that redacts nothing.

Today the engine classifies the CEP-shaped SKUs as CEPs, the invalid eleven-digit codes as
phones, and the product lines, store names, neighbourhoods and cities as people's names, and
counts the two protocols starting with `00` as test rows.

Open rulings: whether CEP-shaped values are kept when the column says otherwise, whether a city,
a company's 0800 line or a product's launch date pass through.
