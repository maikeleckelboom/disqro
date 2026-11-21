//! One reference-copy operation for playlist rows in Library and for the
//! complete viewport of an open playlist, including its empty state.
use super::*;
use std::cell::RefCell;

pub struct DraggedTracks {
    pub targets: Arc<Targets>,
    label: SharedString,
    preview: RefCell<Option<WeakEntity<TrackPreview>>>,
}

impl DraggedTracks {
    pub(super) fn new(targets: Arc<Targets>, label: SharedString) -> Self {
        Self {
            targets,
            label,
            preview: RefCell::default(),
        }
    }
    pub(super) fn start(&self, cx: &mut App) -> Entity<TrackPreview> {
        let preview = cx.new(|_| TrackPreview {
            label: self.label.clone(),
            destination: None,
        });
        self.preview.replace(Some(preview.downgrade()));
        preview
    }
    pub(super) fn copy_to(&self, playlist: PlaylistId, catalog: &Entity<Catalog>, cx: &mut App) {
        if self.targets.can_copy_to(playlist) {
            catalog.update(cx, |c, cx| {
                c.add_to_playlist(playlist, self.targets.as_ref().clone(), cx)
            });
        }
    }
}

pub(super) struct TrackPreview {
    label: SharedString,
    destination: Option<(Bounds<Pixels>, SharedString)>,
}
impl Render for TrackPreview {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let label = self
            .destination
            .as_ref()
            .filter(|(bounds, _)| bounds.contains(&window.mouse_position()))
            .map(|(_, label)| label)
            .unwrap_or(&self.label);
        crate::panels::nav::chip(label.clone(), cx)
            .max_w(px(340.))
            .truncate()
    }
}

pub fn playlist_target(
    element: Stateful<Div>,
    playlist: PlaylistId,
    name: SharedString,
    catalog: Entity<Catalog>,
    cx: &App,
) -> Stateful<Div> {
    let m = *materials(cx);
    element
        .drag_over::<DraggedTracks>(move |style, drag, _, _| {
            if drag.targets.can_copy_to(playlist) {
                style.bg(m.selected).border_color(m.accent)
            } else {
                style
            }
        })
        .on_drag_move::<DraggedTracks>(move |event, _, cx| {
            if !event.bounds.contains(&event.event.position) {
                return;
            }
            let drag = event.drag(cx);
            if !drag.targets.can_copy_to(playlist) {
                return;
            }
            let count = drag.targets.len();
            let preview = drag.preview.borrow().as_ref().and_then(WeakEntity::upgrade);
            if let Some(preview) = preview {
                let bounds = event.bounds;
                if preview
                    .read(cx)
                    .destination
                    .as_ref()
                    .is_none_or(|(old, _)| *old != bounds)
                {
                    let noun = if count == 1 { "track" } else { "tracks" };
                    let label = format!("Add {count} {noun} to {name}").into();
                    preview.update(cx, |p, cx| {
                        p.destination = Some((bounds, label));
                        cx.notify();
                    });
                }
            }
        })
        .on_drop(move |drag: &DraggedTracks, _, cx| drag.copy_to(playlist, &catalog, cx))
}
