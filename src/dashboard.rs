//! Cálculos do painel: rosca por categoria, últimos meses, gastos por dia,
//! comparação com o mês anterior e listas.

use crate::model::{Expense, format_amount, iso_to_br_short, month_name};
use crate::{AppWindow, CategoryRow, DayBar, ExpenseItem, MonthBar, Slice, State, Stat};
use chrono::{Datelike, Local, NaiveDate};
use slint::{Color, ModelRc, SharedString, VecModel};
use std::collections::HashMap;
use std::f64::consts::{PI, TAU};

const R_OUT: f64 = 92.0;
const R_OUT_SELECTED: f64 = 99.0;
const R_IN: f64 = 60.0;
const R_IN_SELECTED: f64 = 56.0;

pub fn shift_month(year: i32, month: u32, delta: i32) -> (i32, u32) {
    let idx = year * 12 + month as i32 - 1 + delta;
    (idx.div_euclid(12), idx.rem_euclid(12) as u32 + 1)
}

pub fn days_in_month(year: i32, month: u32) -> u32 {
    let (ny, nm) = shift_month(year, month, 1);
    NaiveDate::from_ymd_opt(ny, nm, 1).and_then(|d| d.pred_opt()).map(|d| d.day()).unwrap_or(30)
}

fn short_month(m: u32) -> &'static str {
    ["Jan", "Fev", "Mar", "Abr", "Mai", "Jun", "Jul", "Ago", "Set", "Out", "Nov", "Dez"][(m.clamp(1, 12) - 1) as usize]
}

fn weekday_pt(d: NaiveDate) -> &'static str {
    ["Segunda", "Terça", "Quarta", "Quinta", "Sexta", "Sábado", "Domingo"][d.weekday().num_days_from_monday() as usize]
}

/// "R$ 1.235" (sem centavos) para caber nos indicadores.
fn format_int(cents: i64) -> String {
    let s = format_amount((cents + 50) / 100 * 100);
    s.trim_end_matches(",00").to_string()
}

/// Valor curto para as barras: "245", "1,2 mil".
fn compact(cents: i64) -> String {
    let r = cents as f64 / 100.0;
    if r <= 0.0 {
        "–".into()
    } else if r >= 1000.0 {
        format!("{:.1} mil", r / 1000.0).replace('.', ",")
    } else {
        format!("{r:.0}")
    }
}

/// Gasto realizado do mês (gastos planejados ficam de fora até o pagamento ser confirmado).
fn month_total(st: &State, year: i32, month: u32) -> i64 {
    st.store.month_expenses(year, month).iter().filter(|e| !e.pending).map(|e| e.amount_cents).sum()
}

/// Gastos planejados (pagamento ainda não confirmado) do mês.
fn month_planned(st: &State, year: i32, month: u32) -> i64 {
    st.store.month_expenses(year, month).iter().filter(|e| e.pending).map(|e| e.amount_cents).sum()
}

/// Texto do lembrete: "Venceu em 01/11", "Vence hoje", "Vence amanhã", "Vence em 09/11".
fn due_label(date_iso: &str, today: NaiveDate) -> String {
    match NaiveDate::parse_from_str(date_iso, "%Y-%m-%d") {
        Ok(d) if d < today => format!("Venceu em {}", iso_to_br_short(date_iso)),
        Ok(d) if d == today => "Vence hoje".to_string(),
        Ok(d) if Some(d) == today.succ_opt() => "Vence amanhã".to_string(),
        _ => format!("Vence em {}", iso_to_br_short(date_iso)),
    }
}

fn month_income(st: &State, year: i32, month: u32) -> i64 {
    st.store.month_incomes(year, month).iter().map(|e| e.amount_cents).sum()
}

/// Saldo com sinal explícito quando negativo: "-R$ 120,00".
fn format_signed(cents: i64) -> String {
    if cents < 0 {
        format!("-{}", format_amount(-cents))
    } else {
        format_amount(cents)
    }
}

/// Totais por categoria do mês selecionado, do maior para o menor.
fn category_totals(st: &State) -> Vec<(String, i64)> {
    let mut map: HashMap<&str, i64> = HashMap::new();
    for e in st.store.month_expenses(st.year, st.month).into_iter().filter(|e| !e.pending) {
        *map.entry(e.category.as_str()).or_default() += e.amount_cents;
    }
    let mut v: Vec<(String, i64)> = map.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    v
}

/// Caminho SVG (viewbox 200x200) de um setor de anel entre os ângulos a0..a1
/// (radianos, sentido horário a partir do topo).
pub fn sector_path(a0: f64, a1: f64, r_out: f64, r_in: f64) -> String {
    let span = a1 - a0;
    if span >= TAU - 1e-6 {
        let mid = a0 + span / 2.0;
        return format!("{} {}", sector_path(a0, mid, r_out, r_in), sector_path(mid, a1, r_out, r_in));
    }
    let pt = |r: f64, a: f64| (100.0 + r * a.sin(), 100.0 - r * a.cos());
    let large = if span > PI { 1 } else { 0 };
    let (x0, y0) = pt(r_out, a0);
    let (x1, y1) = pt(r_out, a1);
    let (x2, y2) = pt(r_in, a1);
    let (x3, y3) = pt(r_in, a0);
    format!(
        "M {x0:.2} {y0:.2} A {r_out} {r_out} 0 {large} 1 {x1:.2} {y1:.2} L {x2:.2} {y2:.2} A {r_in} {r_in} 0 {large} 0 {x3:.2} {y3:.2} Z"
    )
}

/// Qual categoria está sob o toque (x, y normalizados em -1..1 a partir do centro).
pub fn slice_at(st: &State, x: f32, y: f32) -> Option<String> {
    let (x, y) = (x as f64, y as f64);
    let r = (x * x + y * y).sqrt() * 100.0;
    if !(R_IN_SELECTED - 4.0..=100.0).contains(&r) {
        return None;
    }
    let mut angle = x.atan2(-y);
    if angle < 0.0 {
        angle += TAU;
    }
    let cats = category_totals(st);
    let total: i64 = cats.iter().map(|c| c.1).sum();
    if total <= 0 {
        return None;
    }
    let mut acc = 0.0;
    for (name, cents) in cats {
        acc += cents as f64 / total as f64 * TAU;
        if angle <= acc + 1e-9 {
            return Some(name);
        }
    }
    None
}

pub fn fill(ui: &AppWindow, st: &State) {
    let today = Local::now().date_naive();
    // Todos os lançamentos do mês (inclui planejados) e só os gastos realizados.
    let month_all = st.store.month_expenses(st.year, st.month);
    let month_exp: Vec<&Expense> = month_all.iter().copied().filter(|e| !e.pending).collect();
    let planned_month: i64 = month_all.iter().filter(|e| e.pending).map(|e| e.amount_cents).sum();
    let total: i64 = month_exp.iter().map(|e| e.amount_cents).sum();
    let ndays = days_in_month(st.year, st.month);

    ui.set_month_label(format!("{} {}", month_name(st.month), st.year).into());
    ui.set_month_total(format_amount(total).into());
    ui.set_days_in_month(ndays as i32);

    // ---- Renda e saldo (com o saldo que passou do mês anterior) ----
    let income = month_income(st, st.year, st.month);
    let carry = st.store.balance_before(st.year, st.month);
    let available = income + carry;
    let balance = available - total;
    let (_, prev_m) = shift_month(st.year, st.month, -1);
    let prev_month_name = month_name(prev_m).to_lowercase();
    // Sem renda e sem saldo anterior, o destaque é o total gasto (e não um "saldo" negativo).
    let show_balance = income > 0 || carry != 0;

    ui.set_income_total(format_amount(income).into());
    ui.set_has_income(income > 0);
    ui.set_show_balance(show_balance);
    if show_balance {
        ui.set_balance(format_signed(balance).into());
        ui.set_balance_negative(balance < 0);
    } else {
        ui.set_balance(format_amount(total).into());
        ui.set_balance_negative(false);
    }
    ui.set_has_carry(carry != 0);
    ui.set_carry_negative(carry < 0);
    ui.set_carry_text(
        if carry >= 0 {
            format!("+ {} que sobrou até {prev_month_name}", format_amount(carry))
        } else {
            format!("{} que faltou até {prev_month_name}", format_signed(carry))
        }
        .into(),
    );

    let ratio = if available > 0 {
        total as f64 / available as f64
    } else if total > 0 {
        2.0 // nada disponível e houve gastos: barra cheia em vermelho
    } else {
        0.0
    };
    ui.set_spent_ratio(ratio as f32);
    let base = if carry != 0 { "do disponível (renda + saldo anterior)" } else { "da renda" };
    let spent_text = if !show_balance {
        if total > 0 {
            "Toque em Renda para registrar o que você recebeu e ver seu saldo.".to_string()
        } else {
            "Registre sua renda e seus gastos para acompanhar o saldo.".to_string()
        }
    } else if balance < 0 {
        format!("Os gastos passaram {base} em {}", format_amount(-balance))
    } else {
        format!("{:.0}% {base} já foi gasto · sobram {}", ratio * 100.0, format_amount(balance))
    };
    ui.set_spent_text(spent_text.into());

    // ---- Gastos planejados do mês e saldo previsto ----
    let planned_text = match (planned_month > 0, show_balance) {
        (false, _) => String::new(),
        (true, true) => format!(
            "Planejado no mês: {} · saldo previsto {}",
            format_amount(planned_month),
            format_signed(balance - planned_month)
        ),
        (true, false) => format!("Planejado no mês: {}", format_amount(planned_month)),
    };
    ui.set_planned_text(planned_text.into());

    // ---- Lembrete: pagamentos vencidos ou que vencem nos próximos 3 dias ----
    let limit = (today + chrono::Duration::days(3)).format("%Y-%m-%d").to_string();
    let mut due: Vec<&Expense> = st.store.data.expenses.iter().filter(|e| e.pending && e.date <= limit).collect();
    due.sort_by(|a, b| a.date.cmp(&b.date).then(a.id.cmp(&b.id)));
    ui.set_due_count(due.len() as i32);
    ui.set_due_total(format_amount(due.iter().map(|e| e.amount_cents).sum()).into());
    let due_items: Vec<ExpenseItem> = due
        .iter()
        .take(3)
        .map(|e| {
            let mut it = st.item_for(e);
            it.date = due_label(&e.date, today).into();
            it
        })
        .collect();
    ui.set_due_items(ModelRc::new(VecModel::from(due_items)));

    // Extrato de receitas: o saldo anterior aparece como primeira linha.
    let mut income_items: Vec<ExpenseItem> = Vec::new();
    if carry != 0 {
        income_items.push(ExpenseItem {
            id: SharedString::new(),
            description: format!("Saldo de {prev_month_name}").into(),
            category: "Saldo anterior".into(),
            initial: "S".into(),
            amount: if carry > 0 {
                format!("+ {}", format_amount(carry))
            } else {
                format_signed(carry)
            }
            .into(),
            date: format!("01/{:02}", st.month).into(),
            has_photo: false,
            color: if carry > 0 { Color::from_rgb_u8(0x15, 0x65, 0xc0) } else { Color::from_rgb_u8(0xd3, 0x2f, 0x2f) },
            is_header: false,
            header_total: SharedString::new(),
            is_income: carry > 0,
            planned: false,
            overdue: false,
        });
    }
    income_items.extend(st.store.month_incomes(st.year, st.month).iter().map(|i| st.income_item(i)));
    ui.set_incomes(ModelRc::new(VecModel::from(income_items)));

    // ---- Comparação com o mês anterior ----
    let (py, pm) = shift_month(st.year, st.month, -1);
    let prev = month_total(st, py, pm);
    let prev_name = month_name(pm).to_lowercase();
    let (text, up) = if prev == 0 {
        if total == 0 {
            ("Nenhum gasto registrado".to_string(), false)
        } else {
            (format!("Sem gastos em {prev_name} para comparar"), false)
        }
    } else {
        let pct = (total - prev) as f64 * 100.0 / prev as f64;
        if pct >= 0.5 {
            (format!("{pct:.0}% a mais que em {prev_name}"), true)
        } else if pct <= -0.5 {
            (format!("{:.0}% a menos que em {prev_name}", -pct), false)
        } else {
            (format!("Igual a {prev_name}"), false)
        }
    };
    ui.set_compare_text(text.into());
    ui.set_compare_up(up);

    // ---- Indicadores ----
    let elapsed = if st.year == today.year() && st.month == today.month() {
        today.day()
    } else {
        ndays
    };
    let biggest = month_exp.iter().map(|e| e.amount_cents).max().unwrap_or(0);
    let stats = vec![
        Stat { label: "Despesas".into(), value: month_exp.len().to_string().into() },
        Stat { label: "Média/dia".into(), value: format_int(total / elapsed.max(1) as i64).into() },
        Stat { label: "Maior gasto".into(), value: format_int(biggest).into() },
    ];
    ui.set_stats(ModelRc::new(VecModel::from(stats)));

    // ---- Rosca por categoria ----
    let cats = category_totals(st);
    let selected = cats.iter().any(|c| c.0 == st.selected_slice).then(|| st.selected_slice.clone()).unwrap_or_default();
    let mut slices = Vec::new();
    if total > 0 {
        let gap = if cats.len() > 1 { 0.012 } else { 0.0 };
        let mut a0 = 0.0;
        for (name, cents) in &cats {
            let frac = *cents as f64 / total as f64;
            let a1 = a0 + frac * TAU;
            let is_sel = *name == selected;
            let (ro, ri) = if is_sel { (R_OUT_SELECTED, R_IN_SELECTED) } else { (R_OUT, R_IN) };
            let (s0, s1) = if a1 - a0 > 4.0 * gap { (a0 + gap, a1 - gap) } else { (a0, a1) };
            slices.push(Slice {
                name: name.into(),
                value: format_amount(*cents).into(),
                percent: format!("{:.0}%", frac * 100.0).into(),
                fraction: (*cents as f64 / cats[0].1 as f64) as f32,
                color: st.category_color(name),
                path: sector_path(s0, s1, ro, ri).into(),
                selected: is_sel,
            });
            a0 = a1;
        }
    } else {
        slices.push(Slice {
            name: SharedString::new(),
            value: SharedString::new(),
            percent: SharedString::new(),
            fraction: 0.0,
            color: Color::from_rgb_u8(0xe4, 0xea, 0xe3),
            path: sector_path(0.0, TAU, R_OUT, R_IN).into(),
            selected: false,
        });
    }
    ui.set_has_data(total > 0);
    ui.set_slices(ModelRc::new(VecModel::from(slices)));
    ui.set_selected_slice(selected.clone().into());
    if let Some((name, cents)) = cats.iter().find(|c| c.0 == selected) {
        ui.set_donut_title(name.into());
        ui.set_donut_value(format_amount(*cents).into());
        ui.set_donut_sub(format!("{:.0}% do mês", *cents as f64 * 100.0 / total as f64).into());
    } else {
        ui.set_donut_title("Total".into());
        ui.set_donut_value(format_amount(total).into());
        ui.set_donut_sub(
            match cats.len() {
                0 => "Nenhum gasto".to_string(),
                1 => "1 categoria".to_string(),
                n => format!("{n} categorias"),
            }
            .into(),
        );
    }

    // ---- Últimos 6 meses ----
    let months: Vec<(u32, i64, i64, i32, i64)> = (-5..=0)
        .map(|off| {
            let (y, m) = shift_month(st.year, st.month, off);
            (m, month_total(st, y, m), month_income(st, y, m), off, month_planned(st, y, m))
        })
        .collect();
    let max_m = months.iter().map(|m| (m.1 + m.4).max(m.2)).max().unwrap_or(0).max(1);
    let with_data: Vec<i64> = months.iter().map(|m| m.1).filter(|v| *v > 0).collect();
    let bars: Vec<MonthBar> = months
        .iter()
        .map(|(m, v, inc, off, planned)| MonthBar {
            label: short_month(*m).into(),
            value: compact(*v).into(),
            fraction: *v as f32 / max_m as f32,
            income_fraction: *inc as f32 / max_m as f32,
            planned_fraction: *planned as f32 / max_m as f32,
            selected: *off == 0,
            offset: *off,
        })
        .collect();
    ui.set_month_bars(ModelRc::new(VecModel::from(bars)));
    ui.set_months_info(if with_data.is_empty() {
        SharedString::new()
    } else {
        format!("Média de gastos: {}", format_int(with_data.iter().sum::<i64>() / with_data.len() as i64)).into()
    });

    // ---- Gastos por dia ----
    let mut per_day = vec![0i64; ndays as usize + 1];
    let mut count_day = vec![0usize; ndays as usize + 1];
    for e in &month_exp {
        if let Ok(d) = NaiveDate::parse_from_str(&e.date, "%Y-%m-%d") {
            let i = d.day() as usize;
            if i < per_day.len() {
                per_day[i] += e.amount_cents;
                count_day[i] += 1;
            }
        }
    }
    let max_d = per_day.iter().copied().max().unwrap_or(0).max(1);
    let day_bars: Vec<DayBar> = (1..=ndays as usize)
        .map(|d| DayBar { day: d as i32, fraction: per_day[d] as f32 / max_d as f32 })
        .collect();
    ui.set_day_bars(ModelRc::new(VecModel::from(day_bars)));
    let sel_day = if (1..=ndays as i32).contains(&st.selected_day) { st.selected_day } else { 0 };
    ui.set_selected_day(sel_day);
    let info = if sel_day > 0 {
        let d = sel_day as usize;
        match count_day[d] {
            0 => format!("Dia {d}: nenhum gasto"),
            1 => format!("Dia {d}: {} · 1 despesa", format_amount(per_day[d])),
            n => format!("Dia {d}: {} · {n} despesas", format_amount(per_day[d])),
        }
    } else if total > 0 {
        let (best, v) = per_day.iter().enumerate().max_by_key(|(_, v)| **v).unwrap();
        format!("Maior gasto no dia {best} ({}). Toque numa barra.", format_amount(*v))
    } else {
        "Sem gastos neste mês.".to_string()
    };
    ui.set_day_info(info.into());

    // ---- Maiores despesas ----
    let mut top: Vec<_> = month_exp.clone();
    top.sort_by(|a, b| b.amount_cents.cmp(&a.amount_cents).then(b.id.cmp(&a.id)));
    let top_items: Vec<ExpenseItem> = top.iter().take(5).map(|e| st.item_for(e)).collect();
    ui.set_top_expenses(ModelRc::new(VecModel::from(top_items)));

    // ---- Lista de despesas (aba 2), agrupada por dia ----
    let filtered: Vec<_> = month_all.iter().filter(|e| st.filter.is_empty() || e.category == st.filter).collect();
    let filter_total: i64 = filtered.iter().filter(|e| !e.pending).map(|e| e.amount_cents).sum();
    let mut items: Vec<ExpenseItem> = Vec::new();
    let mut i = 0;
    while i < filtered.len() {
        let date = &filtered[i].date;
        let group: Vec<_> = filtered[i..].iter().take_while(|e| &e.date == date).collect();
        let day_total: i64 = group.iter().filter(|e| !e.pending).map(|e| e.amount_cents).sum();
        let label = match NaiveDate::parse_from_str(date, "%Y-%m-%d") {
            Ok(d) if d == today => format!("Hoje, {}", iso_to_br_short(date)),
            Ok(d) if Some(d) == today.pred_opt() => format!("Ontem, {}", iso_to_br_short(date)),
            Ok(d) => format!("{}, {}", weekday_pt(d), iso_to_br_short(date)),
            Err(_) => date.clone(),
        };
        items.push(ExpenseItem {
            id: SharedString::new(),
            description: label.into(),
            category: SharedString::new(),
            initial: SharedString::new(),
            amount: SharedString::new(),
            date: SharedString::new(),
            has_photo: false,
            color: Color::default(),
            is_header: true,
            header_total: if day_total > 0 || group.iter().any(|e| !e.pending) {
                format_amount(day_total).into()
            } else {
                "planejado".into()
            },
            is_income: false,
            planned: false,
            overdue: false,
        });
        for e in &group {
            items.push(st.item_for(e));
        }
        i += group.len();
    }
    ui.set_expenses(ModelRc::new(VecModel::from(items)));
    ui.set_filter_category(st.filter.clone().into());
    ui.set_filter_total(format_amount(filter_total).into());

    // ---- Aba Categorias ----
    let max_c = cats.first().map(|c| c.1).unwrap_or(0).max(1);
    let rows: Vec<CategoryRow> = st
        .store
        .data
        .categories
        .iter()
        .map(|name| {
            let cents = cats.iter().find(|c| &c.0 == name).map(|c| c.1).unwrap_or(0);
            let n = month_exp.iter().filter(|e| &e.category == name).count();
            CategoryRow {
                name: name.into(),
                total: format_amount(cents).into(),
                info: match n {
                    0 => "Nenhuma despesa no mês".to_string(),
                    1 => "1 despesa no mês".to_string(),
                    n => format!("{n} despesas no mês"),
                }
                .into(),
                color: st.category_color(name),
                fraction: cents as f32 / max_c as f32,
            }
        })
        .collect();
    ui.set_category_rows(ModelRc::new(VecModel::from(rows)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn months() {
        assert_eq!(shift_month(2026, 1, -1), (2025, 12));
        assert_eq!(shift_month(2026, 11, -5), (2026, 6));
        assert_eq!(shift_month(2026, 12, 1), (2027, 1));
        assert_eq!(days_in_month(2026, 2), 28);
        assert_eq!(days_in_month(2028, 2), 29);
        assert_eq!(days_in_month(2026, 11), 30);
    }

    #[test]
    fn values() {
        assert_eq!(compact(24590), "246");
        assert_eq!(compact(123456), "1,2 mil");
        assert_eq!(format_int(24590), "R$ 246");
        assert_eq!(format_int(123456), "R$ 1.235");
    }

    #[test]
    fn full_ring_is_two_arcs() {
        let p = sector_path(0.0, TAU, 92.0, 60.0);
        assert_eq!(p.matches('M').count(), 2);
    }
}
