//! Modelo de dados e persistência (JSON no diretório privado do app).

use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use crate::storage;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Expense {
    pub id: u64,
    pub description: String,
    pub category: String,
    /// Valor em centavos, para evitar erros de arredondamento.
    pub amount_cents: i64,
    /// Data no formato ISO `AAAA-MM-DD`.
    pub date: String,
    /// Nome do arquivo da foto dentro da pasta `fotos/`.
    #[serde(default)]
    pub photo: Option<String>,
    /// Momento da última alteração (ms desde 1970), usado para mesclar aparelhos.
    #[serde(default)]
    pub updated: i64,
    /// Compra parcelada: identificador da compra (o mesmo em todas as parcelas).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<u64>,
    /// Parcela [número, total], ex.: [2, 10] = 2ª de 10.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installment: Option<[u32; 2]>,
    /// Gasto planejado: só vira gasto realizado quando o pagamento é confirmado.
    /// Ausente (falso) nos registros antigos, que continuam sendo gastos realizados.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pending: bool,
}

/// Dinheiro recebido (salário, freelance, etc.).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Income {
    pub id: u64,
    pub description: String,
    pub amount_cents: i64,
    /// Data no formato ISO `AAAA-MM-DD`.
    pub date: String,
    #[serde(default)]
    pub updated: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Data {
    pub categories: Vec<String>,
    pub expenses: Vec<Expense>,
    /// Ausente em arquivos de versões antigas do app.
    #[serde(default)]
    pub incomes: Vec<Income>,
    /// Mantido por compatibilidade; os IDs novos são gerados por `new_id()`.
    pub next_id: u64,
    /// Registros excluídos: id -> momento da exclusão (para propagar entre aparelhos).
    #[serde(default)]
    pub deleted: BTreeMap<u64, i64>,
    /// Momento da última alteração na lista de categorias.
    #[serde(default)]
    pub categories_updated: i64,
}

/// Milissegundos desde 1970. (Via chrono: no navegador `SystemTime` não existe.)
pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// ID único entre aparelhos: tempo em ms nos bits altos + 16 bits aleatórios.
/// Dentro do mesmo app os IDs são sempre crescentes (nunca se repetem).
pub fn new_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static LAST: AtomicU64 = AtomicU64::new(0);
    let mut rnd = [0u8; 2];
    let _ = getrandom::getrandom(&mut rnd);
    let candidate = ((now_ms() as u64) << 16) | u16::from_le_bytes(rnd) as u64;
    let prev = LAST
        .try_update(Ordering::SeqCst, Ordering::SeqCst, |last| Some(candidate.max(last + 1)))
        .unwrap_or(0);
    candidate.max(prev + 1)
}

trait Record: Clone {
    fn id(&self) -> u64;
    fn updated(&self) -> i64;
}
impl Record for Expense {
    fn id(&self) -> u64 {
        self.id
    }
    fn updated(&self) -> i64 {
        self.updated
    }
}
impl Record for Income {
    fn id(&self) -> u64 {
        self.id
    }
    fn updated(&self) -> i64 {
        self.updated
    }
}

fn merge_records<T: Record>(a: &[T], b: &[T], deleted: &BTreeMap<u64, i64>) -> Vec<T> {
    let mut map: BTreeMap<u64, T> = BTreeMap::new();
    for r in a.iter().chain(b.iter()) {
        match map.get(&r.id()) {
            Some(existing) if existing.updated() >= r.updated() => {}
            _ => {
                map.insert(r.id(), r.clone());
            }
        }
    }
    map.into_values()
        .filter(|r| deleted.get(&r.id()).map_or(true, |&t| t < r.updated()))
        .collect()
}

/// Mescla duas cópias dos dados (ex.: celular e computador).
/// Vence a alteração mais recente de cada registro; exclusões se propagam.
pub fn merge(a: &Data, b: &Data) -> Data {
    let mut deleted = a.deleted.clone();
    for (id, t) in &b.deleted {
        let e = deleted.entry(*id).or_insert(*t);
        *e = (*e).max(*t);
    }
    let (categories, categories_updated) = if b.categories_updated > a.categories_updated {
        (b.categories.clone(), b.categories_updated)
    } else {
        (a.categories.clone(), a.categories_updated)
    };
    Data {
        categories,
        expenses: merge_records(&a.expenses, &b.expenses, &deleted),
        incomes: merge_records(&a.incomes, &b.incomes, &deleted),
        next_id: a.next_id.max(b.next_id),
        deleted,
        categories_updated,
    }
}

impl Default for Data {
    fn default() -> Self {
        Data {
            categories: [
                "Alimentação",
                "Moradia",
                "Transporte",
                "Saúde",
                "Educação",
                "Lazer",
                "Contas",
                "Outros",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            expenses: Vec::new(),
            incomes: Vec::new(),
            next_id: 1,
            deleted: BTreeMap::new(),
            categories_updated: 0,
        }
    }
}

pub struct Store {
    dir: PathBuf,
    pub data: Data,
}

impl Store {
    pub fn load(dir: PathBuf) -> Store {
        let data = match storage::read_text(&dir, "gastos.json") {
            Some(text) => match serde_json::from_str::<Data>(&text) {
                Ok(d) => d,
                Err(_) => {
                    // Arquivo corrompido: guarda uma cópia e recomeça, sem perder o original.
                    let _ = storage::write_text(&dir, "gastos.json.corrompido", &text);
                    Data::default()
                }
            },
            None => Data::default(),
        };
        Store { dir, data }
    }

    pub fn save(&self) -> std::io::Result<()> {
        let text = serde_json::to_string_pretty(&self.data)?;
        storage::write_text(&self.dir, "gastos.json", &text)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Bytes da foto (JPEG) guardada com esse nome.
    pub fn photo_bytes(&self, name: &str) -> Option<Vec<u8>> {
        storage::read_photo(&self.dir, name)
    }

    pub fn delete_photo(&self, name: &str) {
        storage::delete_photo(&self.dir, name)
    }

    /// A foto é usada por alguma despesa (fora as indicadas)? As parcelas de uma
    /// compra compartilham a mesma foto, então só se apaga o arquivo quando ninguém mais a usa.
    pub fn photo_in_use(&self, name: &str, except: &[u64]) -> bool {
        self.data.expenses.iter().any(|e| !except.contains(&e.id) && e.photo.as_deref() == Some(name))
    }

    /// Apaga a foto do disco se nenhuma outra despesa a usa.
    pub fn delete_photo_if_unused(&self, name: &str, except: &[u64]) {
        if !self.photo_in_use(name, except) {
            self.delete_photo(name);
        }
    }

    pub fn find(&self, id: u64) -> Option<&Expense> {
        self.data.expenses.iter().find(|e| e.id == id)
    }

    /// Despesas de um mês, mais recentes primeiro.
    pub fn month_expenses(&self, year: i32, month: u32) -> Vec<&Expense> {
        let prefix = format!("{year:04}-{month:02}-");
        let mut v: Vec<&Expense> = self
            .data
            .expenses
            .iter()
            .filter(|e| e.date.starts_with(&prefix))
            .collect();
        v.sort_by(|a, b| b.date.cmp(&a.date).then(b.id.cmp(&a.id)));
        v
    }

    /// Saldo acumulado de todos os meses anteriores a `year/month`
    /// (receitas − despesas). É o valor que "passa" para o mês.
    pub fn balance_before(&self, year: i32, month: u32) -> i64 {
        let first_day = format!("{year:04}-{month:02}-01");
        let inc: i64 = self.data.incomes.iter().filter(|i| i.date < first_day).map(|i| i.amount_cents).sum();
        let exp: i64 = self
            .data
            .expenses
            .iter()
            .filter(|e| !e.pending && e.date < first_day)
            .map(|e| e.amount_cents)
            .sum();
        inc - exp
    }

    pub fn find_income(&self, id: u64) -> Option<&Income> {
        self.data.incomes.iter().find(|e| e.id == id)
    }

    /// Receitas de um mês, mais recentes primeiro.
    pub fn month_incomes(&self, year: i32, month: u32) -> Vec<&Income> {
        let prefix = format!("{year:04}-{month:02}-");
        let mut v: Vec<&Income> = self.data.incomes.iter().filter(|e| e.date.starts_with(&prefix)).collect();
        v.sort_by(|a, b| b.date.cmp(&a.date).then(b.id.cmp(&a.id)));
        v
    }
}

/// Converte texto digitado ("12,50", "1.234,56", "R$ 10", "7.5") em centavos.
pub fn parse_amount(input: &str) -> Option<i64> {
    let s: String = input
        .trim()
        .trim_start_matches("R$")
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    if s.is_empty() {
        return None;
    }
    let normalized = if s.contains(',') {
        // Formato brasileiro: ponto = milhar, vírgula = decimal.
        s.replace('.', "").replace(',', ".")
    } else if s.matches('.').count() > 1 {
        s.replace('.', "")
    } else {
        s
    };
    let value: f64 = normalized.parse().ok()?;
    if !value.is_finite() || value < 0.0 {
        return None;
    }
    Some((value * 100.0).round() as i64)
}

/// Formata centavos como "R$ 1.234,56".
pub fn format_amount(cents: i64) -> String {
    let neg = cents < 0;
    let c = cents.unsigned_abs();
    let int = (c / 100).to_string();
    let mut grouped = String::new();
    for (i, ch) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            grouped.push('.');
        }
        grouped.push(ch);
    }
    format!("{}R$ {},{:02}", if neg { "-" } else { "" }, grouped, c % 100)
}

/// Soma `n` meses a uma data; se o dia não existir no mês de destino, usa o último dia
/// (ex.: 31/01 + 1 mês = 28/02).
pub fn add_months(date: NaiveDate, n: u32) -> NaiveDate {
    date.checked_add_months(chrono::Months::new(n)).unwrap_or(date)
}

/// Divide o total em `n` parcelas; os centavos que sobram vão para a 1ª.
pub fn split_installments(total: i64, n: u32) -> Vec<i64> {
    let n = n.max(1) as i64;
    let base = total / n;
    let mut v = vec![base; n as usize];
    v[0] += total - base * n;
    v
}

/// Dados de uma compra a registrar (à vista se `parcels` for 1).
pub struct NewPurchase<'a> {
    pub description: &'a str,
    pub category: &'a str,
    /// Valor digitado: total da compra (`amount_is_total`) ou valor de cada parcela.
    pub amount: i64,
    pub amount_is_total: bool,
    pub parcels: u32,
    /// Data da compra / 1ª parcela; as demais vencem no mesmo dia dos meses seguintes.
    pub first_date: NaiveDate,
    pub photo: Option<String>,
}

/// Gera as despesas de uma compra: uma por parcela, mensais. Parcelas com vencimento
/// depois de `today` entram como gasto planejado (pagamento a confirmar).
pub fn build_purchase(p: &NewPurchase, today: NaiveDate) -> Vec<Expense> {
    let parcels = p.parcels.clamp(1, 60);
    let values = if parcels > 1 && p.amount_is_total {
        split_installments(p.amount, parcels)
    } else {
        vec![p.amount; parcels as usize]
    };
    let group = (parcels > 1).then(new_id);
    let now = now_ms();
    values
        .into_iter()
        .enumerate()
        .map(|(i, value)| {
            let due = add_months(p.first_date, i as u32);
            Expense {
                id: new_id(),
                description: p.description.to_string(),
                category: p.category.to_string(),
                amount_cents: value,
                date: due.format("%Y-%m-%d").to_string(),
                photo: p.photo.clone(),
                updated: now,
                group,
                installment: (parcels > 1).then_some([i as u32 + 1, parcels]),
                pending: starts_pending(due, today),
            }
        })
        .collect()
}

/// Uma despesa nova começa como "planejada" (pagamento a confirmar) quando a data
/// é posterior a hoje. A parcela paga no dia da compra já entra como gasto.
pub fn starts_pending(date: NaiveDate, today: NaiveDate) -> bool {
    date > today
}

/// Converte "dd/mm/aaaa" (ou "dd/mm", usando o ano informado) em data.
pub fn parse_date_br(input: &str, default_year: i32) -> Option<NaiveDate> {
    let parts: Vec<&str> = input.trim().split(['/', '-', '.']).collect();
    let (d, m, y) = match parts.as_slice() {
        [d, m] => (d.parse().ok()?, m.parse().ok()?, default_year),
        [d, m, y] => {
            let y: i32 = y.parse().ok()?;
            (d.parse().ok()?, m.parse().ok()?, if y < 100 { 2000 + y } else { y })
        }
        _ => return None,
    };
    NaiveDate::from_ymd_opt(y, m, d)
}

pub fn format_date_br(date: &NaiveDate) -> String {
    format!("{:02}/{:02}/{:04}", date.day(), date.month(), date.year())
}

pub fn iso_to_br_short(iso: &str) -> String {
    match NaiveDate::parse_from_str(iso, "%Y-%m-%d") {
        Ok(d) => format!("{:02}/{:02}", d.day(), d.month()),
        Err(_) => iso.to_string(),
    }
}

pub fn month_name(month: u32) -> &'static str {
    [
        "Janeiro", "Fevereiro", "Março", "Abril", "Maio", "Junho", "Julho", "Agosto", "Setembro",
        "Outubro", "Novembro", "Dezembro",
    ][(month.clamp(1, 12) - 1) as usize]
}

/// Converte a foto escolhida (bytes JPEG/PNG/WebP) em um JPEG reduzido e
/// já rotacionado conforme o EXIF.
pub fn encode_photo(bytes: &[u8]) -> Result<Vec<u8>, String> {
    use image::{codecs::jpeg::JpegEncoder, DynamicImage, ImageDecoder, ImageReader};
    use std::io::Cursor;

    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    let mut decoder = reader
        .into_decoder()
        .map_err(|_| "Formato de imagem não suportado (use JPEG, PNG ou WebP).".to_string())?;
    let orientation = decoder.orientation().ok();
    let mut img = DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?;
    if let Some(o) = orientation {
        img.apply_orientation(o);
    }
    const MAX: u32 = 1600;
    if img.width() > MAX || img.height() > MAX {
        img = img.resize(MAX, MAX, image::imageops::FilterType::Triangle);
    }
    let rgb = img.to_rgb8();
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, 85)
        .encode_image(&rgb)
        .map_err(|e| e.to_string())?;
    Ok(out)
}

/// Converte, reduz e grava a foto escolhida com o nome indicado.
pub fn import_photo(bytes: &[u8], dir: &Path, name: &str) -> Result<(), String> {
    let jpeg = encode_photo(bytes)?;
    storage::write_photo(dir, name, &jpeg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amounts() {
        assert_eq!(parse_amount("12,50"), Some(1250));
        assert_eq!(parse_amount("1.234,56"), Some(123456));
        assert_eq!(parse_amount("R$ 10"), Some(1000));
        assert_eq!(parse_amount("7.5"), Some(750));
        assert_eq!(parse_amount("1.000.000"), Some(100000000));
        assert_eq!(parse_amount("abc"), None);
        assert_eq!(parse_amount(""), None);
        assert_eq!(format_amount(123456), "R$ 1.234,56");
        assert_eq!(format_amount(5), "R$ 0,05");
        assert_eq!(format_amount(100000000), "R$ 1.000.000,00");
    }

    #[test]
    fn dates() {
        assert_eq!(parse_date_br("06/11/2026", 2000), NaiveDate::from_ymd_opt(2026, 11, 6));
        assert_eq!(parse_date_br("6/11", 2026), NaiveDate::from_ymd_opt(2026, 11, 6));
        assert_eq!(parse_date_br("31/02/2026", 2026), None);
        assert_eq!(iso_to_br_short("2026-11-06"), "06/11");
    }

    fn exp(id: u64, amount: i64, updated: i64) -> Expense {
        Expense {
            id,
            description: format!("e{id}"),
            category: "Outros".into(),
            amount_cents: amount,
            date: "2026-11-01".into(),
            photo: None,
            updated,
            group: None,
            installment: None,
            pending: false,
        }
    }

    #[test]
    fn installments() {
        assert_eq!(split_installments(100000, 3), vec![33334, 33333, 33333]);
        assert_eq!(split_installments(1000, 1), vec![1000]);
        assert_eq!(split_installments(999, 4).iter().sum::<i64>(), 999);
        let d = |s: &str| NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap();
        assert_eq!(add_months(d("2026-01-31"), 1), d("2026-02-28"));
        assert_eq!(add_months(d("2026-11-15"), 2), d("2027-01-15"));
        let today = d("2026-11-06");
        assert!(!starts_pending(d("2026-11-06"), today)); // pago hoje = gasto
        assert!(!starts_pending(d("2026-10-01"), today)); // passado = gasto
        assert!(starts_pending(d("2026-11-20"), today)); // ainda vai pagar = planejado
        assert!(starts_pending(d("2027-01-15"), today));
    }

    #[test]
    fn purchase_in_installments() {
        let d = |s: &str| NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap();
        let today = d("2026-11-06");
        let p = NewPurchase {
            description: "TV",
            category: "Lazer",
            amount: 100000, // R$ 1.000,00 no total
            amount_is_total: true,
            parcels: 3,
            first_date: d("2026-11-06"),
            photo: Some("foto.jpg".into()),
        };
        let v = build_purchase(&p, today);
        assert_eq!(v.len(), 3);
        // valores somam o total; datas mensais; parcela de hoje = gasto, as outras = planejado
        assert_eq!(v.iter().map(|e| e.amount_cents).sum::<i64>(), 100000);
        let got: Vec<(&str, bool, Option<[u32; 2]>)> = v.iter().map(|e| (e.date.as_str(), e.pending, e.installment)).collect();
        assert_eq!(
            got,
            vec![
                ("2026-11-06", false, Some([1, 3])),
                ("2026-12-06", true, Some([2, 3])),
                ("2027-01-06", true, Some([3, 3])),
            ]
        );
        // mesma compra (grupo), IDs diferentes, foto compartilhada
        assert!(v.iter().all(|e| e.group.is_some() && e.group == v[0].group));
        assert!(v[0].id != v[1].id && v[1].id != v[2].id);
        assert!(v.iter().all(|e| e.photo.as_deref() == Some("foto.jpg")));

        // valor por parcela; 1ª parcela já no futuro = tudo planejado
        let p2 = NewPurchase { amount: 5000, amount_is_total: false, first_date: d("2026-11-20"), ..p };
        let v2 = build_purchase(&p2, today);
        assert!(v2.iter().all(|e| e.amount_cents == 5000 && e.pending));

        // à vista, hoje = gasto, sem dados de parcela
        let p3 = NewPurchase { parcels: 1, amount: 4990, first_date: today, ..p2 };
        let v3 = build_purchase(&p3, today);
        assert_eq!(v3.len(), 1);
        assert_eq!((v3[0].pending, v3[0].group, v3[0].installment), (false, None, None));
    }

    #[test]
    fn old_expense_without_installment_fields_loads() {
        let json = r#"{"id":1,"description":"x","category":"Outros","amount_cents":5,"date":"2026-11-01"}"#;
        let e: Expense = serde_json::from_str(json).unwrap();
        assert_eq!((e.group, e.installment, e.pending), (None, None, false));
        // e não grava os campos vazios (arquivo continua compatível com versões antigas)
        let json_out = serde_json::to_string(&e).unwrap();
        assert!(!json_out.contains("installment") && !json_out.contains("pending"));
    }

    #[test]
    fn merge_two_devices() {
        let mut phone = Data::default();
        let mut pc = Data::default();
        // cada aparelho cria um registro
        phone.expenses.push(exp(1, 100, 10));
        pc.expenses.push(exp(2, 200, 11));
        // o mesmo registro editado nos dois: vence o mais recente
        phone.expenses.push(exp(3, 300, 50));
        pc.expenses.push(exp(3, 333, 60));
        // registro excluído no PC depois da última edição no celular
        phone.expenses.push(exp(4, 400, 20));
        pc.deleted.insert(4, 30);
        // registro editado no celular DEPOIS de ser excluído no PC: permanece
        phone.expenses.push(exp(5, 500, 90));
        pc.deleted.insert(5, 80);
        // categorias: vence a lista alterada por último
        pc.categories = vec!["Casa".into()];
        pc.categories_updated = 99;

        let m = merge(&phone, &pc);
        let got: Vec<(u64, i64)> = m.expenses.iter().map(|e| (e.id, e.amount_cents)).collect();
        assert_eq!(got, vec![(1, 100), (2, 200), (3, 333), (5, 500)]);
        assert_eq!(m.categories, vec!["Casa".to_string()]);

        // mesclar é simétrico e idempotente
        assert_eq!(merge(&pc, &phone).expenses, m.expenses);
        assert_eq!(merge(&m, &phone), m);
        assert_eq!(merge(&m, &m), m);
    }

    #[test]
    fn ids_are_unique() {
        let ids: std::collections::HashSet<u64> = (0..10_000).map(|_| new_id()).collect();
        assert_eq!(ids.len(), 10_000);
    }

    #[test]
    fn old_file_without_incomes_loads() {
        let json = r#"{"categories":["Outros"],"expenses":[],"next_id":3}"#;
        let d: Data = serde_json::from_str(json).unwrap();
        assert!(d.incomes.is_empty());
        assert_eq!(d.next_id, 3);
    }

    #[test]
    fn month_filter() {
        let mut s = Store { dir: std::env::temp_dir(), data: Data::default() };
        for (id, d) in [(1, "2026-11-01"), (2, "2026-10-31"), (3, "2026-11-20")] {
            s.data.expenses.push(Expense {
                id,
                description: "x".into(),
                category: "Outros".into(),
                amount_cents: 100,
                date: d.into(),
                photo: None,
                updated: 0,
                group: None,
                installment: None,
                pending: false,
            });
        }
        let ids: Vec<u64> = s.month_expenses(2026, 11).iter().map(|e| e.id).collect();
        assert_eq!(ids, vec![3, 1]);
        s.data.incomes.push(Income { id: 9, description: "Salário".into(), amount_cents: 500000, date: "2026-11-05".into(), updated: 0 });
        s.data.incomes.push(Income { id: 10, description: "Extra".into(), amount_cents: 1000, date: "2026-10-05".into(), updated: 0 });
        let inc: Vec<u64> = s.month_incomes(2026, 11).iter().map(|e| e.id).collect();
        assert_eq!(inc, vec![9]);
    }

    #[test]
    fn carry_over_balance() {
        let mut s = Store { dir: std::env::temp_dir(), data: Data::default() };
        let exp = |id, amount, date: &str| Expense {
            id,
            description: "x".into(),
            category: "Outros".into(),
            amount_cents: amount,
            date: date.into(),
            photo: None,
            updated: 0,
            group: None,
            installment: None,
            pending: false,
        };
        let inc = |id, amount, date: &str| Income { id, description: "Salário".into(), amount_cents: amount, date: date.into(), updated: 0 };
        // Setembro: recebe 1000, gasta 700 -> sobra 300
        s.data.incomes.push(inc(1, 100000, "2026-09-05"));
        s.data.expenses.push(exp(2, 70000, "2026-09-30"));
        // Outubro: recebe 1000, gasta 1500 -> mês fecha em -500, acumulado -200
        s.data.incomes.push(inc(3, 100000, "2026-10-01"));
        s.data.expenses.push(exp(4, 150000, "2026-10-31"));
        // Novembro (não entra no saldo anterior de novembro)
        s.data.expenses.push(exp(5, 999, "2026-11-01"));

        assert_eq!(s.balance_before(2026, 9), 0);
        assert_eq!(s.balance_before(2026, 10), 30000);
        assert_eq!(s.balance_before(2026, 11), -20000);
        // gasto planejado (pagamento não confirmado) não reduz o saldo
        let mut planejado = exp(6, 50000, "2026-10-15");
        planejado.pending = true;
        s.data.expenses.push(planejado);
        assert_eq!(s.balance_before(2026, 11), -20000);
        // Virada de ano
        assert_eq!(s.balance_before(2027, 1), -20000 - 999);
    }
}
