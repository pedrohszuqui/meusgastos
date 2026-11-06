//! Meus Gastos — controle de gastos mensais por categoria, com foto opcional.
//! UI em Slint; roda no Android (APK via cargo-apk), no computador e no navegador
//! (WebAssembly, inclusive iPhone/iPad como app da tela de início).

mod cloud;
mod dashboard;
mod model;
mod storage;
#[cfg(not(target_arch = "wasm32"))]
mod sync;
#[cfg(target_os = "android")]
mod android;
#[cfg(target_arch = "wasm32")]
mod web;

use chrono::{Datelike, Duration, Local, NaiveDate};
use model::{format_amount, Expense, Store};
use slint::{Color, ComponentHandle, Image, ModelRc, SharedString, VecModel};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

slint::include_modules!();

const PALETTE: [(u8, u8, u8); 10] = [
    (0x2e, 0x7d, 0x32), // verde
    (0x1e, 0x88, 0xe5), // azul
    (0xfb, 0x8c, 0x00), // laranja
    (0xd8, 0x1b, 0x60), // rosa
    (0x8e, 0x24, 0xaa), // roxo
    (0x00, 0xac, 0xc1), // ciano
    (0xe5, 0x39, 0x35), // vermelho
    (0x7c, 0xb3, 0x42), // verde-claro
    (0x6d, 0x4c, 0x41), // marrom
    (0x54, 0x6e, 0x7a), // cinza-azulado
];

pub(crate) struct State {
    store: Store,
    year: i32,
    month: u32,
    filter: String,
    selected_slice: String,
    selected_day: i32,
    editing: Option<u64>,
    /// `true` quando o item em edição é uma receita.
    editing_income: bool,
    /// Foto atualmente associada ao formulário (nome do arquivo em `fotos/`).
    form_photo: Option<String>,
    /// Fotos importadas durante esta edição (apagadas se o usuário cancelar).
    new_photos: Vec<String>,
    /// Incrementado a cada alteração salva (para saber se algo mudou durante a sincronização).
    revision: Cell<u64>,
}

impl State {
    pub(crate) fn category_color(&self, name: &str) -> Color {
        let idx = self
            .store
            .data
            .categories
            .iter()
            .position(|c| c == name)
            .unwrap_or(PALETTE.len() - 1);
        let (r, g, b) = PALETTE[idx % PALETTE.len()];
        Color::from_rgb_u8(r, g, b)
    }

    fn discard_new_photos(&mut self, keep: Option<&str>) {
        for p in self.new_photos.drain(..) {
            if Some(p.as_str()) != keep {
                self.store.delete_photo(&p);
            }
        }
    }

    fn set_month(&mut self, year: i32, month: u32) {
        self.year = year;
        self.month = month;
        self.filter.clear();
        self.selected_slice.clear();
        self.selected_day = 0;
    }

    fn shift_month(&mut self, delta: i32) {
        let (y, m) = dashboard::shift_month(self.year, self.month, delta);
        self.set_month(y, m);
    }

    pub(crate) fn item_for(&self, e: &Expense) -> ExpenseItem {
        let today = Local::now().format("%Y-%m-%d").to_string();
        let description = match e.installment {
            Some([n, total]) => format!("{} ({n}/{total})", e.description),
            None => e.description.clone(),
        };
        ExpenseItem {
            id: e.id.to_string().into(),
            description: description.into(),
            category: e.category.clone().into(),
            initial: e.category.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default().into(),
            amount: format_amount(e.amount_cents).into(),
            date: model::iso_to_br_short(&e.date).into(),
            has_photo: e.photo.is_some(),
            color: self.category_color(&e.category),
            is_header: false,
            header_total: SharedString::new(),
            is_income: false,
            planned: e.pending,
            overdue: e.pending && e.date < today,
        }
    }

    pub(crate) fn income_item(&self, e: &model::Income) -> ExpenseItem {
        ExpenseItem {
            id: e.id.to_string().into(),
            description: e.description.clone().into(),
            category: "Receita".into(),
            initial: "$".into(),
            amount: format!("+ {}", format_amount(e.amount_cents)).into(),
            date: model::iso_to_br_short(&e.date).into(),
            has_photo: false,
            color: Color::from_rgb_u8(0x15, 0x65, 0xc0),
            is_header: false,
            header_total: SharedString::new(),
            is_income: true,
            planned: false,
            overdue: false,
        }
    }
}

fn load_image(state: &State, name: &str) -> Image {
    state.store.photo_bytes(name).map(|b| image_from_bytes(&b)).unwrap_or_default()
}

/// Decodifica a foto (JPEG/PNG) para exibição. Funciona igual no Android, PC e navegador.
fn image_from_bytes(bytes: &[u8]) -> Image {
    match image::load_from_memory(bytes) {
        Ok(img) => {
            let rgba = img.to_rgba8();
            Image::from_rgba8(slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                rgba.as_raw(),
                rgba.width(),
                rgba.height(),
            ))
        }
        Err(_) => Image::default(),
    }
}

fn refresh(ui: &AppWindow, st: &State) {
    dashboard::fill(ui, st);

    let chips: Vec<CatChip> = st
        .store
        .data
        .categories
        .iter()
        .map(|c| CatChip { name: c.into(), color: st.category_color(c) })
        .collect();
    ui.set_chips(ModelRc::new(VecModel::from(chips)));
}

fn save_or_warn(ui: &AppWindow, st: &State) -> bool {
    match st.store.save() {
        Ok(()) => {
            st.revision.set(st.revision.get() + 1);
            cloud::changed();
            true
        }
        Err(e) => {
            ui.set_form_error(format!("Erro ao salvar: {e}").into());
            false
        }
    }
}

/// Abre o formulário vazio. `kind`: 0 = despesa, 1 = receita.
fn open_new_form(ui: &AppWindow, st: &mut State, kind: i32) {
    st.editing = None;
    st.editing_income = false;
    st.form_photo = None;
    st.new_photos.clear();
    let today = Local::now().date_naive();
    let date = if today.year() == st.year && today.month() == st.month {
        today
    } else {
        NaiveDate::from_ymd_opt(st.year, st.month, 1).unwrap_or(today)
    };
    let preferred = if ui.get_tab() == 1 { st.filter.clone() } else { st.selected_slice.clone() };
    let cat_idx = st.store.data.categories.iter().position(|c| *c == preferred).unwrap_or(0);
    ui.set_form_kind(kind);
    ui.set_editing_id(-1);
    ui.set_form_description("".into());
    ui.set_form_amount("".into());
    ui.set_form_category_index(cat_idx as i32);
    ui.set_form_date(model::format_date_br(&date).into());
    ui.set_form_has_photo(false);
    ui.set_form_photo(Image::default());
    ui.set_form_error("".into());
    ui.set_confirm_delete(false);
    ui.set_form_parcelado(false);
    ui.set_form_parcels(2);
    ui.set_form_total_mode(true);
    ui.set_form_planned(false);
    ui.set_form_in_group(false);
    ui.set_form_parcel_label("".into());
    ui.set_form_confirm(false);
    ui.set_form_plan_preview("".into());
    ui.set_page(1);
}

/// Prévia mostrada no formulário de nova despesa: parcelamento e/ou aviso de gasto planejado.
fn plan_preview(ui: &AppWindow, default_year: i32) -> String {
    if ui.get_form_kind() != 0 || ui.get_editing_id() >= 0 {
        return String::new();
    }
    let today = Local::now().date_naive();
    let date = model::parse_date_br(&ui.get_form_date(), default_year);
    let amount = model::parse_amount(&ui.get_form_amount()).filter(|v| *v > 0);
    if ui.get_form_parcelado() {
        let n = ui.get_form_parcels().clamp(2, 60) as u32;
        let (Some(amount), Some(date)) = (amount, date) else {
            return format!("{n} parcelas mensais. Informe o valor e a data da 1ª parcela.");
        };
        let values = if ui.get_form_total_mode() {
            model::split_installments(amount, n)
        } else {
            vec![amount; n as usize]
        };
        let total: i64 = values.iter().sum();
        let pending = (0..n).filter(|i| model::starts_pending(model::add_months(date, *i), today)).count() as u32;
        let each = if values.iter().all(|v| *v == values[0]) {
            format!("{n}x de {}", format_amount(values[0]))
        } else {
            format!("1x de {} + {}x de {}", format_amount(values[0]), n - 1, format_amount(values[1]))
        };
        let status = match pending {
            0 => "todas entram como gasto.".to_string(),
            p if p == n => "todas entram como gasto planejado (confirme cada pagamento).".to_string(),
            p => format!("{} entra(m) como gasto agora e {p} como gasto planejado (confirme cada pagamento).", n - p),
        };
        format!(
            "{each} · total {} · de {} a {}: {status}",
            format_amount(total),
            date.format("%m/%Y"),
            model::add_months(date, n - 1).format("%m/%Y")
        )
    } else if date.is_some_and(|d| model::starts_pending(d, today)) {
        "Data futura: entra como gasto planejado até você confirmar o pagamento.".into()
    } else {
        String::new()
    }
}

fn unique_photo_name() -> String {
    // Sufixo aleatório: evita colisão de nomes entre aparelhos sincronizados.
    let now = Local::now();
    format!("foto_{}_{:03}_{:04x}.jpg", now.format("%Y%m%d_%H%M%S"), now.timestamp_subsec_millis(), model::new_id() & 0xffff)
}

/// Abre um endereço no navegador do sistema.
pub(crate) fn open_url(url: &str) {
    #[cfg(target_os = "android")]
    let _ = android::open_url(url);
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("rundll32").args(["url.dll,FileProtocolHandler", url]).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "android")))]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    #[cfg(target_arch = "wasm32")]
    web::open_url(url);
}

pub fn run(data_dir: PathBuf) -> Result<(), slint::PlatformError> {
    let ui = AppWindow::new()?;
    let today = Local::now().date_naive();
    let state = Rc::new(RefCell::new(State {
        store: Store::load(data_dir),
        year: today.year(),
        month: today.month(),
        filter: String::new(),
        selected_slice: String::new(),
        selected_day: 0,
        editing: None,
        editing_income: false,
        form_photo: None,
        new_photos: Vec::new(),
        revision: Cell::new(0),
    }));
    refresh(&ui, &state.borrow());
    cloud::install(&ui, state.clone());
    // Navegador: a página pode ter acabado de voltar do login do Google.
    #[cfg(target_arch = "wasm32")]
    if let Some(result) = web::handle_login_redirect() {
        cloud::login_result(&ui, result);
    }

    // No computador, abre numa janela grande (layout com barra lateral).
    // (No navegador o tamanho é o da página.)
    #[cfg(not(any(target_os = "android", target_arch = "wasm32")))]
    ui.window().set_size(slint::LogicalSize::new(1280.0, 820.0));

    // ---------- Painel / navegação ----------
    let (w, s) = (ui.as_weak(), state.clone());
    ui.on_prev_month(move || {
        let mut st = s.borrow_mut();
        st.shift_month(-1);
        refresh(&w.unwrap(), &st);
    });
    let (w, s) = (ui.as_weak(), state.clone());
    ui.on_next_month(move || {
        let mut st = s.borrow_mut();
        st.shift_month(1);
        refresh(&w.unwrap(), &st);
    });
    let (w, s) = (ui.as_weak(), state.clone());
    ui.on_select_month_offset(move |off| {
        let mut st = s.borrow_mut();
        if off != 0 {
            st.shift_month(off);
            refresh(&w.unwrap(), &st);
        }
    });
    let (w, s) = (ui.as_weak(), state.clone());
    ui.on_select_slice(move |name| {
        let mut st = s.borrow_mut();
        st.selected_slice = name.to_string();
        refresh(&w.unwrap(), &st);
    });
    let (w, s) = (ui.as_weak(), state.clone());
    ui.on_donut_tap(move |x, y| {
        let mut st = s.borrow_mut();
        let hit = dashboard::slice_at(&st, x, y).unwrap_or_default();
        st.selected_slice = if hit == st.selected_slice { String::new() } else { hit };
        refresh(&w.unwrap(), &st);
    });
    let (w, s) = (ui.as_weak(), state.clone());
    ui.on_select_day(move |d| {
        let mut st = s.borrow_mut();
        st.selected_day = d;
        refresh(&w.unwrap(), &st);
    });
    let (w, s) = (ui.as_weak(), state.clone());
    ui.on_set_filter(move |cat| {
        let mut st = s.borrow_mut();
        st.filter = cat.to_string();
        refresh(&w.unwrap(), &st);
    });

    // ---------- Formulário ----------
    let (w, s) = (ui.as_weak(), state.clone());
    ui.on_new_expense(move || open_new_form(&w.unwrap(), &mut s.borrow_mut(), 0));
    let (w, s) = (ui.as_weak(), state.clone());
    ui.on_new_income(move || open_new_form(&w.unwrap(), &mut s.borrow_mut(), 1));

    let (w, s) = (ui.as_weak(), state.clone());
    ui.on_edit_expense(move |id| {
        let ui = w.unwrap();
        let mut st = s.borrow_mut();
        // IDs chegam como texto (u64 não cabe no int da UI). Linhas sintéticas têm id vazio.
        let Ok(id) = id.parse::<u64>() else { return };
        // Receita?
        if let Some(inc) = st.store.find_income(id).cloned() {
            st.editing = Some(inc.id);
            st.editing_income = true;
            st.form_photo = None;
            st.new_photos.clear();
            ui.set_form_kind(1);
            ui.set_editing_id(1);
            ui.set_form_parcelado(false);
            ui.set_form_planned(false);
            ui.set_form_in_group(false);
            ui.set_form_parcel_label("".into());
            ui.set_form_plan_preview("".into());
            ui.set_form_description(inc.description.clone().into());
            ui.set_form_amount(format_amount(inc.amount_cents).trim_start_matches("R$ ").into());
            let date = NaiveDate::parse_from_str(&inc.date, "%Y-%m-%d")
                .map(|d| model::format_date_br(&d))
                .unwrap_or(inc.date.clone());
            ui.set_form_date(date.into());
            ui.set_form_photo(Image::default());
            ui.set_form_has_photo(false);
            ui.set_form_error("".into());
            ui.set_confirm_delete(false);
            ui.set_page(1);
            return;
        }
        let Some(e) = st.store.find(id).cloned() else { return };
        st.editing = Some(e.id);
        st.editing_income = false;
        ui.set_form_kind(0);
        ui.set_form_parcelado(false);
        ui.set_form_planned(e.pending);
        ui.set_form_in_group(e.group.is_some());
        ui.set_form_parcel_label(match e.installment {
            Some([n, total]) => format!("Parcela {n} de {total}").into(),
            None => "".into(),
        });
        ui.set_form_confirm(false);
        ui.set_form_plan_preview("".into());
        st.form_photo = e.photo.clone();
        st.new_photos.clear();
        // Categoria que foi removida da lista continua editável.
        if !st.store.data.categories.contains(&e.category) {
            st.store.data.categories.push(e.category.clone());
            st.store.data.categories_updated = model::now_ms();
            refresh(&ui, &st);
        }
        let cat_idx = st.store.data.categories.iter().position(|c| *c == e.category).unwrap_or(0);
        ui.set_editing_id(1);
        ui.set_form_description(e.description.clone().into());
        ui.set_form_amount(format_amount(e.amount_cents).trim_start_matches("R$ ").into());
        ui.set_form_category_index(cat_idx as i32);
        let date = NaiveDate::parse_from_str(&e.date, "%Y-%m-%d")
            .map(|d| model::format_date_br(&d))
            .unwrap_or(e.date.clone());
        ui.set_form_date(date.into());
        match &e.photo {
            Some(p) => {
                ui.set_form_photo(load_image(&st, p));
                ui.set_form_has_photo(true);
            }
            None => {
                ui.set_form_photo(Image::default());
                ui.set_form_has_photo(false);
            }
        }
        ui.set_form_error("".into());
        ui.set_confirm_delete(false);
        ui.set_page(1);
    });

    let w = ui.as_weak();
    ui.on_set_date_offset(move |off| {
        let d = Local::now().date_naive() + Duration::days(off as i64);
        w.unwrap().set_form_date(model::format_date_br(&d).into());
    });

    let (w, s) = (ui.as_weak(), state.clone());
    ui.on_save_expense(move || {
        let ui = w.unwrap();
        let mut st = s.borrow_mut();
        let Some(amount) = model::parse_amount(&ui.get_form_amount()).filter(|v| *v > 0) else {
            ui.set_form_error("Informe um valor válido, ex.: 49,90".into());
            return;
        };
        let Some(date) = model::parse_date_br(&ui.get_form_date(), st.year) else {
            ui.set_form_error("Data inválida. Use o formato dd/mm/aaaa.".into());
            return;
        };

        // ----- Receita -----
        if ui.get_form_kind() == 1 {
            let mut description = ui.get_form_description().trim().to_string();
            if description.is_empty() {
                description = "Receita".into();
            }
            let iso = date.format("%Y-%m-%d").to_string();
            match (st.editing, st.editing_income) {
                (Some(id), true) => {
                    if let Some(i) = st.store.data.incomes.iter_mut().find(|i| i.id == id) {
                        i.description = description;
                        i.amount_cents = amount;
                        i.date = iso;
                        i.updated = model::now_ms();
                    }
                }
                _ => {
                    st.store.data.incomes.push(model::Income {
                        id: model::new_id(),
                        description,
                        amount_cents: amount,
                        date: iso,
                        updated: model::now_ms(),
                    });
                }
            }
            if !save_or_warn(&ui, &st) {
                return;
            }
            st.discard_new_photos(None);
            if st.year != date.year() || st.month != date.month() {
                st.set_month(date.year(), date.month());
            }
            st.editing = None;
            st.editing_income = false;
            refresh(&ui, &st);
            ui.set_page(0);
            return;
        }

        // ----- Despesa -----
        let idx = ui.get_form_category_index().max(0) as usize;
        let Some(category) = st.store.data.categories.get(idx).cloned() else {
            ui.set_form_error("Escolha uma categoria.".into());
            return;
        };
        let mut description = ui.get_form_description().trim().to_string();
        if description.is_empty() {
            description = category.clone();
        } else {
            // Primeira letra maiúscula, para a lista ficar uniforme.
            let mut c = description.chars();
            if let Some(f) = c.next() {
                description = f.to_uppercase().collect::<String>() + c.as_str();
            }
        }
        let photo = st.form_photo.clone();
        let today = Local::now().date_naive();
        // "Confirmar pagamento" no formulário: salva as edições e marca como pago.
        let confirm = ui.get_form_confirm();
        ui.set_form_confirm(false);
        let old_photo = match st.editing {
            Some(id) => {
                let Some(e) = st.store.data.expenses.iter_mut().find(|e| e.id == id) else { return };
                let old = e.photo.clone();
                e.description = description;
                e.category = category;
                e.amount_cents = amount;
                e.date = date.format("%Y-%m-%d").to_string();
                e.photo = photo.clone();
                if confirm {
                    e.pending = false;
                }
                e.updated = model::now_ms();
                old
            }
            None => {
                // À vista = 1 parcela. Parcelado: uma despesa por mês, a partir da data informada.
                let purchase = model::NewPurchase {
                    description: &description,
                    category: &category,
                    amount,
                    amount_is_total: ui.get_form_total_mode(),
                    parcels: if ui.get_form_parcelado() { ui.get_form_parcels().clamp(2, 60) as u32 } else { 1 },
                    first_date: date,
                    photo: photo.clone(),
                };
                // Parcelas com vencimento futuro entram como gasto planejado até confirmar o pagamento.
                let new = model::build_purchase(&purchase, today);
                st.store.data.expenses.extend(new);
                None
            }
        };
        if !save_or_warn(&ui, &st) {
            return;
        }
        if let Some(old) = old_photo.filter(|o| Some(o) != photo.as_ref()) {
            // As parcelas de uma compra compartilham a foto: só apaga se ninguém mais usa.
            st.store.delete_photo_if_unused(&old, &[]);
        }
        st.discard_new_photos(photo.as_deref());
        // Mostra o mês da despesa salva.
        if st.year != date.year() || st.month != date.month() {
            st.set_month(date.year(), date.month());
        }
        st.editing = None;
        refresh(&ui, &st);
        ui.set_page(0);
    });

    let (w, s) = (ui.as_weak(), state.clone());
    ui.on_delete_expense(move || {
        let ui = w.unwrap();
        let mut st = s.borrow_mut();
        if st.editing_income {
            if let Some(id) = st.editing.take() {
                st.store.data.incomes.retain(|i| i.id != id);
                st.store.data.deleted.insert(id, model::now_ms());
                if !save_or_warn(&ui, &st) {
                    return;
                }
            }
            st.editing_income = false;
        } else if let Some(id) = st.editing.take() {
            if let Some(pos) = st.store.data.expenses.iter().position(|e| e.id == id) {
                let removed = st.store.data.expenses.remove(pos);
                st.store.data.deleted.insert(id, model::now_ms());
                if !save_or_warn(&ui, &st) {
                    return;
                }
                if let Some(p) = removed.photo {
                    st.store.delete_photo_if_unused(&p, &[]);
                }
            }
        }
        st.discard_new_photos(None);
        refresh(&ui, &st);
        ui.set_page(0);
    });

    // Compra parcelada: exclui esta parcela e as seguintes (as anteriores ficam).
    let (w, s) = (ui.as_weak(), state.clone());
    ui.on_delete_future(move || {
        let ui = w.unwrap();
        let mut st = s.borrow_mut();
        let Some(id) = st.editing.take() else { return };
        let Some(e) = st.store.find(id).cloned() else { return };
        let ids: Vec<u64> = match (e.group, e.installment) {
            (Some(g), Some([n, _])) => st
                .store
                .data
                .expenses
                .iter()
                .filter(|x| x.group == Some(g) && x.installment.is_some_and(|[k, _]| k >= n))
                .map(|x| x.id)
                .collect(),
            _ => vec![id],
        };
        let photos: Vec<String> = st
            .store
            .data
            .expenses
            .iter()
            .filter(|x| ids.contains(&x.id))
            .filter_map(|x| x.photo.clone())
            .collect();
        st.store.data.expenses.retain(|x| !ids.contains(&x.id));
        let now = model::now_ms();
        for i in &ids {
            st.store.data.deleted.insert(*i, now);
        }
        if !save_or_warn(&ui, &st) {
            return;
        }
        for p in photos {
            st.store.delete_photo_if_unused(&p, &[]);
        }
        st.discard_new_photos(None);
        refresh(&ui, &st);
        ui.set_page(0);
    });

    // Lembrete do Painel: confirma o pagamento de um gasto planejado.
    let (w, s) = (ui.as_weak(), state.clone());
    ui.on_confirm_payment(move |id| {
        let ui = w.unwrap();
        let mut st = s.borrow_mut();
        let Ok(id) = id.parse::<u64>() else { return };
        let Some(e) = st.store.data.expenses.iter_mut().find(|e| e.id == id) else { return };
        e.pending = false;
        e.updated = model::now_ms();
        save_or_warn(&ui, &st);
        refresh(&ui, &st);
    });

    // Prévia do parcelamento / aviso de data futura, atualizada enquanto o formulário é preenchido.
    let (w, s) = (ui.as_weak(), state.clone());
    ui.on_update_preview(move || {
        let ui = w.unwrap();
        let Ok(st) = s.try_borrow() else { return };
        ui.set_form_plan_preview(plan_preview(&ui, st.year).into());
    });

    let (w, s) = (ui.as_weak(), state.clone());
    ui.on_cancel_form(move || {
        let ui = w.unwrap();
        let mut st = s.borrow_mut();
        st.discard_new_photos(None);
        st.editing = None;
        st.editing_income = false;
        ui.set_page(0);
    });

    // ---------- Fotos ----------
    let (w, s) = (ui.as_weak(), state.clone());
    ui.on_photo_imported(move |name| {
        let ui = w.unwrap();
        let mut st = s.borrow_mut();
        let name = name.to_string();
        st.new_photos.push(name.clone());
        st.form_photo = Some(name.clone());
        ui.set_form_photo(load_image(&st, &name));
        ui.set_form_has_photo(true);
        ui.set_form_error("".into());
        ui.set_busy(false);
        ui.set_page(1);
    });

    let w = ui.as_weak();
    ui.on_photo_failed(move |msg| {
        let ui = w.unwrap();
        ui.set_busy(false);
        ui.set_form_error(msg.clone());
        ui.set_picker_status(msg);
    });

    let (w, s) = (ui.as_weak(), state.clone());
    ui.on_remove_photo(move || {
        let ui = w.unwrap();
        s.borrow_mut().form_photo = None;
        ui.set_form_has_photo(false);
        ui.set_form_photo(Image::default());
    });

    let w = ui.as_weak();
    ui.on_view_photo(move || {
        let ui = w.unwrap();
        ui.set_viewer_image(ui.get_form_photo());
        ui.set_page(3);
    });

    let (w, s) = (ui.as_weak(), state.clone());
    ui.on_pick_photo(move || {
        let ui = w.unwrap();
        let dir = s.borrow().store.dir().to_path_buf();
        platform_pick_photo(&ui, dir, unique_photo_name());
    });

    #[cfg(target_os = "android")]
    {
        let w = ui.as_weak();
        ui.on_picker_refresh(move || android::open_picker(w.clone()));
        let w = ui.as_weak();
        ui.on_close_picker(move || {
            android::cancel_picker();
            let ui = w.unwrap();
            ui.set_picker_items(ModelRc::default());
            ui.set_page(1);
        });
        let (w, s) = (ui.as_weak(), state.clone());
        ui.on_picker_select(move |i| {
            let dir = s.borrow().store.dir().to_path_buf();
            android::import_selected(w.clone(), i as usize, dir, unique_photo_name());
        });
    }

    // ---------- Categorias ----------
    let (w, s) = (ui.as_weak(), state.clone());
    ui.on_add_category(move |name| {
        let ui = w.unwrap();
        let mut st = s.borrow_mut();
        let name = name.trim().to_string();
        if name.is_empty() {
            ui.set_category_error("Digite o nome da categoria.".into());
            return;
        }
        if st.store.data.categories.iter().any(|c| c.to_lowercase() == name.to_lowercase()) {
            ui.set_category_error("Essa categoria já existe.".into());
            return;
        }
        st.store.data.categories.push(name);
        st.store.data.categories_updated = model::now_ms();
        ui.set_category_error("".into());
        save_or_warn(&ui, &st);
        refresh(&ui, &st);
    });

    let (w, s) = (ui.as_weak(), state.clone());
    ui.on_remove_category(move |name| {
        let ui = w.unwrap();
        let mut st = s.borrow_mut();
        if st.store.data.categories.len() <= 1 {
            ui.set_category_error("É preciso manter pelo menos uma categoria.".into());
            return;
        }
        st.store.data.categories.retain(|c| c.as_str() != name.as_str());
        st.store.data.categories_updated = model::now_ms();
        if st.filter == name.as_str() {
            st.filter.clear();
        }
        if st.selected_slice == name.as_str() {
            st.selected_slice.clear();
        }
        ui.set_category_error("".into());
        save_or_warn(&ui, &st);
        refresh(&ui, &st);
    });

    // No navegador o laço de eventos continua depois que `run` retorna (ou lança a
    // exceção de controle do winit): mantém a janela viva.
    #[cfg(target_arch = "wasm32")]
    {
        web::fit_to_page(ui.as_weak());
        std::mem::forget(ui.clone_strong());
    }
    ui.run()
}

// ---------- Seleção de foto por plataforma ----------

#[cfg(target_os = "android")]
fn platform_pick_photo(ui: &AppWindow, _dir: PathBuf, _name: String) {
    ui.set_picker_status("".into());
    ui.set_page(2);
    android::open_picker(ui.as_weak());
}

#[cfg(target_arch = "wasm32")]
fn platform_pick_photo(ui: &AppWindow, dir: PathBuf, name: String) {
    web::pick_photo(ui.as_weak(), dir, name);
}

#[cfg(not(any(target_os = "android", target_arch = "wasm32")))]
fn platform_pick_photo(ui: &AppWindow, dir: PathBuf, name: String) {
    let Some(path) = rfd::FileDialog::new()
        .set_title("Escolher foto")
        .add_filter("Imagens", &["jpg", "jpeg", "png", "webp"])
        .pick_file()
    else {
        return;
    };
    ui.set_busy(true);
    let w = ui.as_weak();
    std::thread::spawn(move || {
        let result = std::fs::read(&path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| model::import_photo(&bytes, &dir, &name));
        let _ = w.upgrade_in_event_loop(move |ui| match result {
            Ok(()) => ui.invoke_photo_imported(name.into()),
            Err(e) => ui.invoke_photo_failed(format!("Não foi possível abrir a foto: {e}").into()),
        });
    });
}

// ---------- Pontos de entrada ----------

#[cfg(not(any(target_os = "android", target_arch = "wasm32")))]
pub fn run_desktop() {
    let base = std::env::var_os("APPDATA")
        .or_else(|| std::env::var_os("XDG_DATA_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."));
    let dir = std::env::var_os("GASTOS_DATA_DIR").map(PathBuf::from).unwrap_or(base.join("MeusGastos"));
    run(dir).expect("falha ao iniciar a interface");
}

/// Navegador, passo 1 (chamado pelo index.html): carrega as fotos guardadas no IndexedDB.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub async fn prepare() {
    if let Err(e) = storage::init().await {
        web_sys::console::warn_2(&"Fotos offline indisponíveis:".into(), &e);
    }
}

/// Navegador, passo 2: abre o app no `<canvas id="canvas">` da página.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn start() -> Result<(), wasm_bindgen::JsValue> {
    // Erros internos aparecem no console do navegador (em vez de só travar a página).
    std::panic::set_hook(Box::new(|info| web_sys::console::error_1(&format!("Meus Gastos: {info}").into())));
    run(PathBuf::from("/meusgastos")).map_err(|e| wasm_bindgen::JsValue::from_str(&e.to_string()))
}

#[cfg(target_os = "android")]
#[no_mangle]
fn android_main(app: slint::android::AndroidApp) {
    let dir = app
        .internal_data_path()
        .unwrap_or_else(|| PathBuf::from("/data/data/br.com.meusgastos/files"));
    android::set_activity(app.activity_as_ptr());
    slint::android::init(app).expect("falha ao iniciar o Slint no Android");
    run(dir).expect("falha ao iniciar a interface");
}
