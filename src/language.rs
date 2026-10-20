//! The Language dialog: choosing the interface language, and translating the app into it with a
//! local AI model in Ollama.

use crate::app::App;
use crate::dialogs::Dialog;
use crate::i18n::{self, t, tf, Language, Translation};
use eframe::egui::{self, RichText};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

pub struct LanguageDialog {
    languages: Vec<Language>,
    /// The installed Ollama models, or why they couldn't be listed; `None` while looking.
    models: Option<Result<Vec<String>, String>>,
}

/// A translation running in the background.
pub struct Job {
    pub language: Language,
    stop: AtomicBool,
    done: AtomicUsize,
    total: AtomicUsize,
}

impl Job {
    /// How far through it is, from 0 to 1.
    pub fn fraction(&self) -> f32 {
        let total = self.total.load(Ordering::Relaxed);
        if total == 0 { 0.0 } else { self.done.load(Ordering::Relaxed) as f32 / total as f32 }
    }
}

/// What a translation job made.
struct Translated {
    translation: Translation,
    /// How many pieces of text are still in English.
    left: usize,
    /// The job was stopped before it finished.
    stopped: bool,
}

impl App {
    /// The menu item that opens the Language dialog. In another language it also says
    /// "Language" in English, so anyone who chose a language by mistake can find their way back.
    pub fn language_menu_label(&self) -> String {
        with_english(t("language.menu"))
    }

    pub fn open_language(&mut self) {
        if self.dialogs.iter().any(|d| matches!(d.dialog, Dialog::Language(_))) {
            return;
        }
        let dialog = LanguageDialog { languages: i18n::available(&i18n::languages_dir()), models: None };
        let id = self.push(Dialog::Language(Box::new(dialog)));
        self.find_models(id);
    }

    fn find_models(&mut self, dialog_id: u64) {
        self.spawn_plain(i18n::models, move |app, r| {
            if let Some(Dialog::Language(d)) = app.dialog_mut(dialog_id) {
                d.models = Some(r.map_err(|e| format!("{e:#}")));
            }
        });
    }

    /// The model to translate with: the one chosen, if it is still installed, or else the first.
    fn translation_model(&self, models: &[String]) -> Option<String> {
        models.iter().find(|m| **m == self.settings.translation_model).or(models.first()).cloned()
    }

    fn save_settings(&mut self) {
        if let Err(e) = self.settings.save() {
            log::error!("Could not save settings: {e:#}");
        }
    }

    /// Switches the interface to `language`, if it has been translated.
    fn choose_language(&mut self, language: &Language) {
        self.settings.language = language.code.clone();
        self.save_settings();
        let label = language.label();
        if language.code == i18n::ENGLISH_CODE {
            i18n::activate(None);
            self.language_changed();
            self.status = tf("language.chosen", &[("language", &label)]);
            return;
        }
        let dir = i18n::languages_dir();
        if !i18n::file_for(&dir, &language.code).exists() {
            // Stay in the current language until there is a translation.
            self.status = tf("language.not_yet", &[("language", &label)]);
            return;
        }
        match i18n::load(&dir, &language.code) {
            Ok(translation) => {
                // Switch first, so the message is in the new language.
                i18n::activate(Some(&translation));
                self.language_changed();
                let missing = translation.missing().len();
                self.status = if missing == 0 {
                    tf("language.chosen", &[("language", &label)])
                } else {
                    tf("language.partly", &[("language", &label), ("count", &missing)])
                };
            }
            Err(e) => {
                log::warn!("Could not load translation {}: {e:#}", language.code);
                self.error(&t("language.title"), tf("language.load_failed", &[("language", &label), ("error", &format!("{e:#}"))]));
            }
        }
        log::info!("Interface language: {}", language.code);
    }

    /// Translates the interface into `language` with `model`. Only the text not yet translated
    /// is sent, unless everything already has been and the user asks to start again.
    fn translate_app(&mut self, language: Language, model: String) {
        let dir = i18n::languages_dir();
        let file = i18n::file_for(&dir, &language.code);
        let translation = match i18n::load(&dir, &language.code) {
            Ok(translation) => translation,
            Err(e) if file.exists() => {
                // A file that can't be read is replaced, as it was going to be shown in English.
                log::warn!("Translation {} is damaged, starting again: {e:#}", language.code);
                Translation::new(&language)
            }
            Err(_) => Translation::new(&language),
        };
        if translation.missing().is_empty() {
            let text = tf("language.again_question", &[("language", &language.label())]);
            self.push(Dialog::confirm(&t("language.again_title"), &text, move |app| {
                let mut fresh = translation;
                fresh.strings.clear();
                app.start_translation(language, model, fresh);
            }));
        } else {
            self.start_translation(language, model, translation);
        }
    }

    fn start_translation(&mut self, language: Language, model: String, translation: Translation) {
        let job = Arc::new(Job {
            language: language.clone(),
            stop: AtomicBool::new(false),
            done: AtomicUsize::new(0),
            total: AtomicUsize::new(0),
        });
        self.translation = Some(job.clone());
        self.status = tf("language.translating", &[("language", &language.label()), ("model", &model)]);
        let ctx = self.ctx.clone();
        self.spawn_plain(move || translate(&job, &model, translation, &ctx), move |app, r| app.translated(r));
    }

    fn translated(&mut self, result: anyhow::Result<Translated>) {
        let Some(job) = self.translation.take() else { return };
        let label = job.language.label();
        match result {
            Ok(done) => {
                // Use what was translated, even if the job was stopped part way, then report in
                // the new language.
                if self.settings.language == job.language.code {
                    i18n::activate(Some(&done.translation));
                    self.language_changed();
                }
                self.status = if done.stopped {
                    t("language.stopped")
                } else if done.left == 0 {
                    tf("language.done", &[("language", &label)])
                } else {
                    tf("language.done_partly", &[("language", &label), ("count", &done.left)])
                };
                log::info!("Translated into {}; {} piece(s) left in English", job.language.code, done.left);
            }
            Err(e) => {
                // Batches done before it failed were saved, so use them as stopping would.
                if self.settings.language == job.language.code
                    && let Ok(saved) = i18n::load(&i18n::languages_dir(), &job.language.code)
                {
                    i18n::activate(Some(&saved));
                    self.language_changed();
                }
                self.error(&t("language.title"), tf("language.failed", &[("error", &format!("{e:#}"))]));
            }
        }
    }

    pub fn show_language(&mut self, ctx: &egui::Context, modal: egui::Modal, id: u64, d: &mut LanguageDialog, first: bool, escape: bool) -> bool {
        let mut close = escape;
        let mut chosen: Option<Language> = None;
        let mut translate: Option<(Language, String)> = None;
        let mut look_again = false;
        let job = self.translation.clone();
        let current = d.languages.iter().position(|l| l.code == self.settings.language).unwrap_or(0);
        let language = d.languages[current].clone();
        modal.show(ctx, |ui| {
            ui.set_width(520.0);
            ui.heading(t("language.title"));
            ui.add_space(6.0);
            egui::Grid::new("language").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                // The label also says "Language" in English, as the menu item does.
                let label = ui.label(with_english(t("language.label")));
                let mut index = current;
                // The language can't change while it is being translated.
                let r = ui
                    .add_enabled_ui(job.is_none(), |ui| {
                        egui::ComboBox::from_id_salt("language-choice")
                            .selected_text(language.label())
                            .width(320.0)
                            .show_ui(ui, |ui| {
                                for (i, l) in d.languages.iter().enumerate() {
                                    ui.selectable_value(&mut index, i, l.label());
                                }
                            })
                            .response
                    })
                    .inner
                    .labelled_by(label.id);
                if first {
                    r.request_focus();
                }
                if index != current {
                    chosen = Some(d.languages[index].clone());
                }
                ui.end_row();
                if language.code == i18n::ENGLISH_CODE {
                    return;
                }
                let label = ui.label(t("language.model"));
                match &d.models {
                    None => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(t("language.looking"));
                        });
                    }
                    Some(Err(_)) => {
                        ui.label(t("language.no_ollama_short"));
                    }
                    Some(Ok(models)) if models.is_empty() => {
                        ui.label(t("language.no_models_short"));
                    }
                    Some(Ok(models)) => {
                        let model = self.translation_model(models).unwrap_or_default();
                        let mut pick = model.clone();
                        egui::ComboBox::from_id_salt("language-model")
                            .selected_text(&pick)
                            .width(320.0)
                            .show_ui(ui, |ui| {
                                for m in models {
                                    ui.selectable_value(&mut pick, m.clone(), m);
                                }
                            })
                            .response
                            .labelled_by(label.id);
                        if pick != model {
                            self.settings.translation_model = pick;
                            self.save_settings();
                        }
                    }
                }
                ui.end_row();
            });
            if language.code != i18n::ENGLISH_CODE {
                ui.add_space(8.0);
                let model = match &d.models {
                    Some(Ok(models)) => self.translation_model(models),
                    _ => None,
                };
                match &job {
                    Some(job) => {
                        ui.add(egui::ProgressBar::new(job.fraction()).show_percentage());
                        let stopping = job.stop.load(Ordering::Relaxed);
                        if ui.add_enabled(!stopping, egui::Button::new(t("language.stop"))).clicked() {
                            job.stop.store(true, Ordering::Relaxed);
                        }
                    }
                    None => {
                        let button = tf("language.translate", &[("language", &language.label())]);
                        if ui.add_enabled(model.is_some(), egui::Button::new(button)).clicked()
                            && let Some(model) = model
                        {
                            translate = Some((language.clone(), model));
                        }
                    }
                }
                ui.add_space(6.0);
                match &d.models {
                    Some(Err(e)) => {
                        ui.add(egui::Label::new(tf("language.no_ollama", &[("error", e)])).wrap());
                        look_again |= ui.button(t("language.look_again")).clicked();
                    }
                    Some(Ok(models)) if models.is_empty() => {
                        ui.add(egui::Label::new(t("language.no_models")).wrap());
                        look_again |= ui.button(t("language.look_again")).clicked();
                    }
                    _ => {}
                }
                let folder = i18n::languages_dir();
                ui.add(egui::Label::new(RichText::new(tf("language.note", &[("folder", &folder.display())])).small()).wrap());
            }
            ui.add_space(10.0);
            close |= ui.button(t("dialog.close")).clicked();
        });
        if let Some(language) = chosen {
            self.choose_language(&language);
        }
        if let Some((language, model)) = translate {
            self.translate_app(language, model);
        }
        if look_again {
            d.models = None;
            self.find_models(id);
        }
        !close
    }
}

/// `label` followed by "(Language)" while the app is in another language.
fn with_english(label: String) -> String {
    if i18n::translated() { format!("{label} (Language)") } else { label }
}

/// Translates the pieces of interface text that `translation` is missing, a batch at a time, with
/// the Ollama model `model` (runs on a worker thread). What has been translated is saved after
/// every batch, so stopping, or closing the app, keeps it, and the next run carries on from there.
fn translate(job: &Job, model: &str, mut translation: Translation, ctx: &egui::Context) -> anyhow::Result<Translated> {
    let language = &job.language;
    let keys = translation.missing();
    let batches: Vec<&[&str]> = keys.chunks(i18n::BATCH_SIZE).collect();
    log::info!("Translating {} piece(s) of text into {} with {model}", keys.len(), language.code);
    translation.made_with = model.to_owned();
    job.total.store(batches.len(), Ordering::Relaxed);
    let dir = i18n::languages_dir();
    let mut failed_batches = 0;
    let mut last_error = None;
    let mut stopped = false;
    for (i, batch) in batches.iter().enumerate() {
        if job.stop.load(Ordering::Relaxed) {
            stopped = true;
            break;
        }
        // Small models sometimes reply with something that isn't JSON, so each batch gets a
        // second try.
        let result = i18n::translate_batch(model, language, batch).or_else(|e| {
            log::warn!("Translation batch {} failed, trying again: {e:#}", i + 1);
            if job.stop.load(Ordering::Relaxed) { Err(e) } else { i18n::translate_batch(model, language, batch) }
        });
        match result {
            Ok(done) => {
                translation.strings.extend(done);
                i18n::save(&dir, &language.code, &translation)?;
            }
            Err(e) => {
                failed_batches += 1;
                // Every batch so far has failed: Ollama has probably stopped, or the model can't
                // do this. Stop wasting time.
                if failed_batches == i + 1 && failed_batches >= 2 {
                    return Err(e);
                }
                log::warn!("Translation batch {} failed: {e:#}", i + 1);
                last_error = Some(e);
            }
        }
        job.done.store(i + 1, Ordering::Relaxed);
        ctx.request_repaint();
    }
    // Nothing worked at all (a single batch that failed twice): say why, rather than report a
    // translation with nothing in it.
    if !stopped && failed_batches > 0 && failed_batches == batches.len()
        && let Some(e) = last_error
    {
        return Err(e);
    }
    i18n::save(&dir, &language.code, &translation)?;
    let left = translation.missing().len();
    Ok(Translated { translation, left, stopped })
}
