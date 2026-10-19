//! Runs a multi-step workflow, such as onboarding or offboarding a user, one step at a time, and
//! shows its progress and final report.
//!
//! Each step names the earlier steps it needs. A step whose needs didn't all succeed is not run
//! and is reported as skipped, so a failure never leads on to a step that depends on it (such as
//! removing a licence after converting the mailbox failed). Every step's result is recorded, and
//! the workflow is only reported as successful when every step succeeded.

use crate::app::{App, Bold};
use crate::graph::Graph;
use eframe::egui::{self, Align, Layout, RichText};
use egui_extras::{Column, TableBuilder};
use std::sync::{Arc, Mutex};

/// Something a wizard loads in the background.
pub enum Loadable<T> {
    Loading,
    Failed(String),
    Ready(T),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    Pending,
    Running,
    Done(String),
    Failed(String),
    /// Not run, with why.
    Skipped(String),
}

pub struct StepStatus {
    pub label: String,
    pub state: State,
}

#[derive(Default)]
pub struct Progress {
    pub steps: Vec<StepStatus>,
    pub finished: bool,
}

pub type Shared = Arc<Mutex<Progress>>;

type StepFn<G, C> = Box<dyn FnOnce(&G, &mut C) -> anyhow::Result<String> + Send>;

/// One step: what it does, the earlier steps (by index) that must succeed first, and the work,
/// which returns what to report.
pub struct Step<G, C> {
    pub label: String,
    pub needs: Vec<usize>,
    run: StepFn<G, C>,
}

impl<G, C> Step<G, C> {
    pub fn new(label: impl Into<String>, needs: Vec<usize>, run: impl FnOnce(&G, &mut C) -> anyhow::Result<String> + Send + 'static) -> Self {
        Step { label: label.into(), needs, run: Box::new(run) }
    }
}

/// A shared progress list for `steps`, all pending.
pub fn progress_for<G, C>(steps: &[Step<G, C>]) -> Shared {
    Arc::new(Mutex::new(Progress {
        steps: steps.iter().map(|s| StepStatus { label: s.label.clone(), state: State::Pending }).collect(),
        finished: false,
    }))
}

/// Runs `steps` in order (on a worker thread), recording each one's result in `progress` and
/// calling `changed` after each.
pub fn execute<G, C>(g: &G, ctx: &mut C, steps: Vec<Step<G, C>>, progress: &Shared, changed: &dyn Fn()) {
    let mut ok = vec![false; steps.len()];
    for (i, step) in steps.into_iter().enumerate() {
        let blocker = step.needs.iter().find(|&&n| n >= i || !ok[n]).copied();
        let set = |state: State| {
            progress.lock().unwrap().steps[i].state = state;
            changed();
        };
        if let Some(n) = blocker {
            let reason = match progress.lock().unwrap().steps.get(n) {
                Some(s) if n < i => format!("Not run, because \"{}\" did not succeed", s.label),
                _ => "Not run".to_string(),
            };
            log::info!("Workflow step skipped: {}: {reason}", step.label);
            set(State::Skipped(reason));
            continue;
        }
        set(State::Running);
        log::info!("Workflow step: {}", step.label);
        match (step.run)(g, ctx) {
            Ok(text) => {
                ok[i] = true;
                set(State::Done(text));
            }
            Err(e) => {
                log::error!("Workflow step failed: {}: {e:#}", step.label);
                set(State::Failed(format!("{e:#}")));
            }
        }
    }
    progress.lock().unwrap().finished = true;
    changed();
}

/// Counts of steps that succeeded, failed and weren't run.
pub fn counts(p: &Progress) -> (usize, usize, usize) {
    let n = |f: fn(&State) -> bool| p.steps.iter().filter(|s| f(&s.state)).count();
    (n(|s| matches!(s, State::Done(_))), n(|s| matches!(s, State::Failed(_))), n(|s| matches!(s, State::Skipped(_))))
}

/// A one-line summary of how the workflow went, which only says it succeeded if every step did.
pub fn summary(p: &Progress) -> String {
    let (done, failed, skipped) = counts(p);
    if !p.finished {
        let at = p.steps.iter().position(|s| matches!(s.state, State::Running | State::Pending)).map_or(0, |i| i + 1);
        return format!("Working… step {at} of {}", p.steps.len());
    }
    if failed == 0 && skipped == 0 {
        format!("Completed: all {done} steps succeeded.")
    } else {
        format!("Not completed: {done} succeeded, {failed} failed, {skipped} not run. Review the failures below.")
    }
}

fn state_text(state: &State) -> (&'static str, String) {
    match state {
        State::Pending => ("Waiting", String::new()),
        State::Running => ("Running…", String::new()),
        State::Done(t) => ("✔ Done", t.clone()),
        State::Failed(t) => ("✖ Failed", t.clone()),
        State::Skipped(t) => ("Not run", t.clone()),
    }
}

/// The report as rows for a CSV file.
pub fn report_rows(p: &Progress) -> Vec<Vec<String>> {
    p.steps
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let (result, details) = state_text(&s.state);
            vec![(i + 1).to_string(), s.label.clone(), result.trim_start_matches(['✔', '✖', ' ']).to_string(), details]
        })
        .collect()
}

/// The report as plain text, for the clipboard.
pub fn report_text(title: &str, p: &Progress) -> String {
    let mut out = format!("{title}\n{}\n\n", summary(p));
    for row in report_rows(p) {
        out.push_str(&format!("{}. {} — {}", row[0], row[1], row[2]));
        if !row[3].is_empty() {
            out.push_str(&format!(": {}", row[3]));
        }
        out.push('\n');
    }
    out
}

pub const REPORT_HEADERS: [&str; 4] = ["Step", "Action", "Result", "Details"];

/// The progress of each step, then its result.
pub fn progress_ui(ui: &mut egui::Ui, id: u64, p: &Progress) {
    let (_, failed, skipped) = counts(p);
    let text = summary(p);
    let text = if p.finished && failed + skipped > 0 {
        RichText::new(format!("⚠ {text}")).color(ui.visuals().error_fg_color).bold()
    } else {
        RichText::new(text).bold()
    };
    ui.label(text);
    ui.add_space(4.0);
    let row_h = ui.text_style_height(&egui::TextStyle::Body) + 6.0;
    let error = ui.visuals().error_fg_color;
    let weak = ui.visuals().weak_text_color();
    TableBuilder::new(ui)
        .id_salt(("workflow", id))
        .striped(true)
        .resizable(true)
        .max_scroll_height(300.0)
        .min_scrolled_height(300.0)
        .auto_shrink([false, false])
        .cell_layout(Layout::left_to_right(Align::Center))
        .column(Column::auto().at_least(24.0))
        .column(Column::initial(300.0).clip(true))
        .column(Column::initial(80.0))
        .column(Column::remainder().clip(true))
        .header(row_h, |mut h| {
            for t in ["", "Action", "Result", "Details"] {
                h.col(|ui| {
                    ui.label(RichText::new(t).bold());
                });
            }
        })
        .body(|body| {
            body.rows(row_h, p.steps.len(), |mut row| {
                let i = row.index();
                let step = &p.steps[i];
                let (result, details) = state_text(&step.state);
                let color = match step.state {
                    State::Failed(_) => Some(error),
                    State::Skipped(_) | State::Pending => Some(weak),
                    _ => None,
                };
                let paint = |t: &str| {
                    let r = RichText::new(t);
                    match color {
                        Some(c) => r.color(c),
                        None => r,
                    }
                };
                row.col(|ui| {
                    ui.label(format!("{}", i + 1));
                });
                row.col(|ui| {
                    ui.add(egui::Label::new(paint(&step.label)).truncate()).on_hover_text(step.label.as_str());
                });
                row.col(|ui| {
                    if step.state == State::Running {
                        ui.spinner();
                    } else {
                        ui.label(paint(result));
                    }
                });
                row.col(|ui| {
                    let r = ui.add(egui::Label::new(paint(&details)).truncate());
                    if !details.is_empty() {
                        r.on_hover_text(details.as_str());
                    }
                });
            });
        });
}

/// The buttons under a finished report. Returns true when Close was clicked.
pub fn report_buttons(app: &mut App, ui: &mut egui::Ui, title: &str, file_name: &str, p: &Shared) -> bool {
    let finished = p.lock().unwrap().finished;
    let mut close = false;
    ui.horizontal(|ui| {
        if !finished {
            ui.spinner();
            ui.label("Working… don't close the app until every step has finished.");
            return;
        }
        if ui.button("Save report…").clicked() {
            let rows = report_rows(&p.lock().unwrap());
            if let Some(path) = app.save_csv("Save report", file_name, &REPORT_HEADERS, &rows) {
                app.status = format!("Saved the report to {}", path.display());
            }
        }
        if ui.button("Copy report").clicked() {
            ui.ctx().copy_text(report_text(title, &p.lock().unwrap()));
            app.status = "Copied the report to the clipboard".into();
        }
        close = ui.button("  Close  ").clicked();
    });
    close
}

impl App {
    /// Starts running `steps` with `ctx` on a worker thread, then gives `done` the context back.
    /// Returns the progress to show.
    pub fn run_workflow<C: Send + 'static>(
        &mut self,
        label: &str,
        mut ctx: C,
        steps: Vec<Step<Graph, C>>,
        done: impl FnOnce(&mut App, C, &Progress) + Send + 'static,
    ) -> Shared {
        let progress = progress_for(&steps);
        let shared = progress.clone();
        let repaint = self.ctx.clone();
        log::info!("Workflow started: {label} ({} steps)", steps.len());
        let title = label.to_string();
        self.spawn(
            format!("{label}…"),
            move |g| {
                execute(g, &mut ctx, steps, &shared, &|| repaint.request_repaint());
                Ok(ctx)
            },
            {
                let progress = progress.clone();
                move |app, r: anyhow::Result<C>| {
                    let p = progress.lock().unwrap();
                    app.status = format!("{title}: {}", summary(&p));
                    log::info!("Workflow finished: {}", app.status);
                    if let Ok(ctx) = r {
                        done(app, ctx, &p);
                    }
                }
            },
        );
        progress
    }
}

/// A heading for a wizard page: its title and where it is among the pages.
pub fn page_heading(ui: &mut egui::Ui, title: &str, pages: &[&str], current: usize) {
    ui.heading(title);
    ui.horizontal_wrapped(|ui| {
        for (i, p) in pages.iter().enumerate() {
            if i > 0 {
                ui.label(RichText::new("›").weak());
            }
            let text = RichText::new(format!("{}. {p}", i + 1));
            ui.label(if i == current { text.bold().underline() } else { text.weak() });
        }
    });
    ui.separator();
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::bail;

    fn run(steps: Vec<Step<(), Vec<usize>>>) -> (Progress, Vec<usize>) {
        let p = progress_for(&steps);
        let mut ran = Vec::new();
        execute(&(), &mut ran, steps, &p, &|| {});
        let p = Arc::try_unwrap(p).ok().unwrap().into_inner().unwrap();
        (p, ran)
    }

    fn ok(i: usize, needs: Vec<usize>) -> Step<(), Vec<usize>> {
        Step::new(format!("step {i}"), needs, move |_, ran: &mut Vec<usize>| {
            ran.push(i);
            Ok("fine".into())
        })
    }

    #[test]
    fn skips_steps_whose_needs_failed() {
        let fail = Step::new("convert mailbox", vec![], |_, ran: &mut Vec<usize>| {
            ran.push(1);
            bail!("Exchange said no")
        });
        // 2 needs the failed step, 3 needs 2 (so it is skipped too), 4 needs nothing.
        let (p, ran) = run(vec![ok(0, vec![]), fail, ok(2, vec![1]), ok(3, vec![2]), ok(4, vec![0])]);
        assert_eq!(ran, [0, 1, 4]);
        assert_eq!(p.steps[1].state, State::Failed("Exchange said no".into()));
        assert_eq!(p.steps[2].state, State::Skipped("Not run, because \"convert mailbox\" did not succeed".into()));
        assert_eq!(p.steps[3].state, State::Skipped("Not run, because \"step 2\" did not succeed".into()));
        assert_eq!(counts(&p), (2, 1, 2));
        assert!(p.finished);
        assert!(summary(&p).starts_with("Not completed: 2 succeeded, 1 failed, 2 not run"));
    }

    #[test]
    fn only_reports_success_when_every_step_succeeded() {
        let (p, ran) = run(vec![ok(0, vec![]), ok(1, vec![0])]);
        assert_eq!(ran, [0, 1]);
        assert_eq!(summary(&p), "Completed: all 2 steps succeeded.");
        let text = report_text("Offboard Ann", &p);
        assert!(text.contains("1. step 0 — Done: fine"), "{text}");
    }

    #[test]
    fn a_step_cannot_need_a_later_one() {
        let (p, ran) = run(vec![ok(0, vec![1]), ok(1, vec![])]);
        assert_eq!(ran, [1]);
        assert_eq!(p.steps[0].state, State::Skipped("Not run".into()));
    }
}
