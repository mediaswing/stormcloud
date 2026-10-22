//! The results list as shown: which of a node's objects pass the filter, in what order, and which
//! one is selected. It knows nothing of drawing, so what it does can be tested on its own.

use crate::model::Column;
use serde_json::Value;

pub struct ListView {
    /// What the filter box holds.
    pub filter: String,
    /// The column sorted on, and whether in ascending order.
    pub sort: Option<(usize, bool)>,
    /// The selected position among the rows shown.
    pub sel: Option<usize>,
    /// The filter, sort or objects changed, so the rows shown must be worked out again before
    /// they are used.
    pub dirty: bool,
    /// The positions in the node's objects of the rows shown, in order.
    rows: Vec<usize>,
    /// The selected object's ID, taken before the objects changed, as the rows shown no longer
    /// match the objects until they are worked out again.
    sel_before_change: Option<Option<Value>>,
}

impl ListView {
    pub fn new() -> ListView {
        ListView { filter: String::new(), sort: None, sel: None, dirty: true, rows: Vec::new(), sel_before_change: None }
    }

    /// Starts afresh, for another node's objects.
    pub fn reset(&mut self) {
        *self = ListView::new();
    }

    /// Forgets the selection kept from before the objects changed, as they have all gone.
    pub fn forget_objects(&mut self) {
        self.sel_before_change = None;
        self.dirty = true;
    }

    /// The positions in `objects` of the rows shown, in order.
    pub fn rows(&self) -> &[usize] {
        &self.rows
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The objects shown, in order.
    pub fn shown<'a>(&self, objects: &'a [Value]) -> Vec<&'a Value> {
        self.rows.iter().filter_map(|&i| objects.get(i)).collect()
    }

    pub fn selected<'a>(&self, objects: &'a [Value]) -> Option<&'a Value> {
        objects.get(*self.rows.get(self.sel?)?)
    }

    /// Selects the row at `pos`, or the last row if there are fewer. Returns false when there are
    /// no rows to select.
    pub fn select(&mut self, pos: usize) -> bool {
        self.sel = if self.rows.is_empty() { None } else { Some(pos.min(self.rows.len() - 1)) };
        self.sel.is_some()
    }

    /// Selects the first row, unless one is selected already.
    pub fn select_first_if_none(&mut self) {
        if self.sel.is_none() && !self.rows.is_empty() {
            self.sel = Some(0);
        }
    }

    /// Sorts on column `col`, ascending, or the other way if it is sorted on already.
    pub fn toggle_sort(&mut self, col: usize) {
        self.sort = match self.sort {
            Some((c, true)) if c == col => Some((col, false)),
            _ => Some((col, true)),
        };
        self.dirty = true;
    }

    /// Remembers which object is selected, before `objects` changes while the rows shown still
    /// match it. A selection already remembered is kept, as that is the one shown.
    pub fn objects_changing(&mut self, objects: &[Value]) {
        if self.sel_before_change.is_none() {
            self.sel_before_change = Some(self.selected(objects).map(|v| v["id"].clone()));
        }
    }

    /// Works out the rows shown from `objects`, under `cols`, leaving out those `hidden` says to.
    /// The selection follows the object selected; when that object has gone, the one now in its
    /// place is selected.
    pub fn rebuild(&mut self, objects: &[Value], cols: &[Column], hidden: impl Fn(&Value) -> bool) {
        let prev = match self.sel_before_change.take() {
            Some(prev) => prev,
            None => self.selected(objects).map(|v| v["id"].clone()),
        };
        let old_pos = self.sel;
        let needle = self.filter.trim().to_lowercase();
        let mut rows: Vec<usize> = (0..objects.len())
            .filter(|&i| !hidden(&objects[i]))
            .filter(|&i| needle.is_empty() || cols.iter().any(|c| (c.get)(&objects[i]).to_lowercase().contains(&needle)))
            .collect();
        if let Some((ci, asc)) = self.sort
            && let Some(col) = cols.get(ci) {
                rows.sort_by_cached_key(|&i| {
                    let text = (col.get)(&objects[i]);
                    // Sort numeric columns numerically.
                    (text.parse::<f64>().ok().map(|n| (n * 1000.0) as i64), text.to_lowercase())
                });
                if !asc {
                    rows.reverse();
                }
            }
        self.sel = prev
            .and_then(|id| rows.iter().position(|&i| objects[i]["id"] == id))
            .or(if rows.is_empty() { None } else { Some(old_pos.unwrap_or(0).min(rows.len() - 1)) });
        self.rows = rows;
        self.dirty = false;
    }

    /// The position of the next row after the selection whose `first` column starts with `typed`,
    /// going round to the top, for jumping to a row by typing.
    pub fn next_starting_with(&self, objects: &[Value], first: fn(&Value) -> String, typed: &str) -> Option<usize> {
        let typed = typed.to_lowercase();
        let n = self.rows.len();
        let cur = self.sel.unwrap_or(0);
        (1..=n).map(|k| (cur + k) % n).find(|&p| first(&objects[self.rows[p]]).to_lowercase().starts_with(&typed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::s;
    use serde_json::json;

    fn cols() -> Vec<Column> {
        vec![
            Column { title: "col.name", width: 0.0, get: |v| s(v, "name") },
            Column { title: "col.total", width: 0.0, get: |v| s(v, "size") },
        ]
    }

    fn objects() -> Vec<Value> {
        vec![
            json!({ "id": "a", "name": "Ann", "size": 10 }),
            json!({ "id": "b", "name": "bob", "size": 9 }),
            json!({ "id": "c", "name": "Cy", "size": 100 }),
            json!({ "id": "d", "name": "Di", "size": "n/a" }),
        ]
    }

    fn names(view: &ListView, objects: &[Value]) -> Vec<String> {
        view.shown(objects).iter().map(|v| s(v, "name")).collect()
    }

    fn built(objects: &[Value], change: impl FnOnce(&mut ListView)) -> ListView {
        let mut view = ListView::new();
        change(&mut view);
        view.rebuild(objects, &cols(), |_| false);
        view
    }

    #[test]
    fn filters_on_any_column_ignoring_case_and_hides_what_it_is_told_to() {
        let objects = objects();
        assert_eq!(names(&built(&objects, |v| v.filter = " B ".into()), &objects), ["bob"]);
        assert_eq!(names(&built(&objects, |v| v.filter = "100".into()), &objects), ["Cy"]);
        let mut view = ListView::new();
        view.rebuild(&objects, &cols(), |v| v["id"] == "a");
        assert_eq!(names(&view, &objects), ["bob", "Cy", "Di"]);
    }

    #[test]
    fn sorts_numbers_as_numbers_and_text_ignoring_case() {
        let objects = objects();
        let by_size = built(&objects, |v| v.toggle_sort(1));
        // Text that isn't a number comes before the numbers.
        assert_eq!(names(&by_size, &objects), ["Di", "bob", "Ann", "Cy"]);
        let by_name_down = built(&objects, |v| {
            v.toggle_sort(0);
            v.toggle_sort(0);
        });
        assert_eq!(by_name_down.sort, Some((0, false)));
        assert_eq!(names(&by_name_down, &objects), ["Di", "Cy", "bob", "Ann"]);
    }

    #[test]
    fn the_selection_follows_its_object_when_the_objects_change() {
        let mut objects = objects();
        let mut view = built(&objects, |_| {});
        view.select(2);
        assert_eq!(view.selected(&objects).unwrap()["id"], "c");
        // Another object is added at the top, so Cy moves down.
        view.objects_changing(&objects);
        objects.insert(0, json!({ "id": "z", "name": "Zed", "size": 1 }));
        view.rebuild(&objects, &cols(), |_| false);
        assert_eq!(view.selected(&objects).unwrap()["id"], "c");
        assert_eq!(view.sel, Some(3));
    }

    #[test]
    fn when_the_selected_object_goes_the_one_in_its_place_is_selected() {
        let mut objects = objects();
        let mut view = built(&objects, |_| {});
        view.select(3);
        view.objects_changing(&objects);
        objects.retain(|v| v["id"] != "d");
        view.rebuild(&objects, &cols(), |_| false);
        assert_eq!(view.selected(&objects).unwrap()["id"], "c");
        // Filtering everything out leaves nothing selected.
        view.filter = "nobody".into();
        view.rebuild(&objects, &cols(), |_| false);
        assert!(view.is_empty() && view.sel.is_none());
        assert!(!view.select(0));
    }

    #[test]
    fn typing_jumps_to_the_next_match_going_round() {
        let objects = vec![
            json!({ "id": "1", "name": "Ann" }),
            json!({ "id": "2", "name": "Bob" }),
            json!({ "id": "3", "name": "Abe" }),
        ];
        let mut view = built(&objects, |_| {});
        let first: fn(&Value) -> String = |v| s(v, "name");
        view.select(0);
        assert_eq!(view.next_starting_with(&objects, first, "a"), Some(2));
        view.select(2);
        assert_eq!(view.next_starting_with(&objects, first, "A"), Some(0));
        assert_eq!(view.next_starting_with(&objects, first, "x"), None);
    }

    #[test]
    fn selecting_past_the_end_selects_the_last_row() {
        let objects = objects();
        let mut view = built(&objects, |_| {});
        assert!(view.select(usize::MAX));
        assert_eq!(view.sel, Some(3));
        view.reset();
        assert!(view.dirty && view.sel.is_none() && view.filter.is_empty());
    }
}
