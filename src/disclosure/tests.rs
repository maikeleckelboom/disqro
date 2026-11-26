use super::*;
use std::cell::Cell;

struct Rows {
    labels: Vec<&'static str>,
    focus: VirtualFocus<&'static str>,
    activated: Rc<Cell<Option<&'static str>>>,
}

impl Render for Rows {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut list = div().flex().flex_col();
        for (index, &label) in self.labels.iter().enumerate() {
            let active = self.activated.clone();
            // Kit supplies the positional wrapper around each virtual item.
            list = list.child(
                div().id(index).child(
                    div()
                        .id(label)
                        .track_focus(&self.focus.handle(label, cx))
                        .child(label)
                        .on_click(move |_, _, _| active.set(Some(label))),
                ),
            );
        }
        list
    }
}

#[::core::prelude::v1::test]
#[ignore = "requires a private native Windows window"]
fn focused_item_survives_insertion_reordering_and_removal_of_its_neighbors() {
    let completed = Rc::new(Cell::new(false));
    let finished = completed.clone();
    gpui_kit::application()
        .with_assets(crate::assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            let active = Rc::new(Cell::new(None));
            let (window, rows) = gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
                cx.new(|_| Rows {
                    labels: vec!["first", "selected", "last"],
                    focus: Default::default(),
                    activated: active.clone(),
                })
            })
            .unwrap();
            cx.spawn(async move |cx| {
                window
                    .update(cx, |_, w, cx| {
                        w.draw(cx).clear(cx);
                        let selected = rows.read(cx).focus.handle("selected", cx);
                        let first = rows.read(cx).focus.handle("first", cx);
                        first.focus(w, cx);
                        w.focus_next(cx);
                        assert!(selected.is_focused(w));
                        for order in [
                            vec!["inserted", "first", "selected", "last"],
                            vec!["last", "selected", "first"],
                            vec!["selected"],
                        ] {
                            rows.update(cx, |rows, cx| {
                                rows.labels = order;
                                cx.notify();
                            });
                            w.refresh();
                            w.draw(cx).clear(cx);
                            assert!(rows.read(cx).focus.handle("selected", cx).is_focused(w));
                            let enter = Keystroke::parse("enter").unwrap();
                            w.dispatch_keystroke(enter.clone(), cx);
                            w.dispatch_event(
                                PlatformInput::KeyUp(KeyUpEvent { keystroke: enter }),
                                cx,
                            );
                            assert_eq!(active.take(), Some("selected"));
                        }
                        rows.update(cx, |r, cx| {
                            r.labels = vec!["replacement"];
                            cx.notify();
                        });
                        w.refresh();
                        w.draw(cx).clear(cx);
                        assert!(!rows.read(cx).focus.handle("replacement", cx).is_focused(w));
                        finished.set(true);
                    })
                    .unwrap();
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
    assert!(completed.get());
}
