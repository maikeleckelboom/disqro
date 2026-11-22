//! Panel types and panel instances.
//!
//! The workspace distinguishes five things:
//!
//! 1. A **panel type** ([`PanelType`]): what kind of view exists, its default
//!    title and icon, and its instance policy. Types are registered once, in
//!    [`TYPES`], and each one is installed in GPUI Kit's dock registry so a
//!    saved layout can rebuild it by name.
//! 2. A **panel instance** ([`Pane`]): one independently configured
//!    occurrence of a type, with a stable [`InstanceId`] that survives
//!    rearranging, saving and restoring.
//! 3. **Dock placement**: where the dock keeps the instance. The dock owns
//!    that tree; nothing here duplicates it.
//! 4. **Panel-local state**: navigation, scroll and view
//!    settings, owned by the instance's [`PaneContent`] and saved with it.
//! 5. **Shared state** ([`Services`]): the catalog, playback and queue, and the
//!    Locations tree. Closing or moving a panel never touches it.
//!
//! Adding a panel type is a new module with a `PanelType` and a
//! [`PaneContent`] implementation, plus one line in [`TYPES`]. The workspace
//! creates, places, closes, saves and restores it without knowing the type.
pub mod browser;
pub mod library;
pub mod locations;
pub mod nav;
pub mod queue;

use crate::{
    browser::{Browser, Location},
    catalog::Catalog,
    collection::ViewPrefs,
    player::Player,
    table::Layouts,
};
use gpui_kit::{
    assets::IconName,
    component::{
        Icon,
        dock::{BasePanel, PanelEvent, PanelInfo, PanelState, TabGroup},
        input::{InputEvent, InputState},
        menu::PopupMenu,
        v_flex,
    },
    prelude::*,
    *,
};
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, rc::Rc};

/// A panel instance's identity. Saved with the layout and never reused for
/// another instance while this one exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct InstanceId(pub u64);
impl InstanceId {
    /// A new identity none of `taken` uses.
    pub fn fresh(taken: impl Fn(InstanceId) -> bool) -> Self {
        loop {
            let id = Self(fastrand::u64(1..u64::MAX >> 11));
            if !taken(id) {
                return id;
            }
        }
    }
}

/// How many instances of a type the panel picker creates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Instances {
    /// The picker reveals the open instance instead of adding another.
    Single,
    /// Every pick opens a new, independent instance.
    Multiple,
}

/// Where a type opens when nothing else decides: beside the content on the
/// leading side, with the content, or on the trailing side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Home {
    Leading,
    Main,
    Trailing,
}

/// One kind of panel.
pub struct PanelType {
    /// The name saved layouts know the type by. Never change it once shipped.
    pub key: &'static str,
    pub title: &'static str,
    pub icon: IconName,
    pub instances: Instances,
    pub home: Home,
    /// Builds an instance from its saved record; a fresh one gets a default
    /// record.
    pub build: fn(&Services, &PanelRecord, &mut Window, &mut App) -> Rc<dyn PaneContent>,
}
impl PartialEq for PanelType {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

/// Every type this build provides, in the order the panel picker lists them.
pub static TYPES: &[&PanelType] = &[
    &browser::TYPE,
    &library::TYPE,
    &locations::TYPE,
    &queue::QUEUE,
    &queue::HISTORY,
];

/// What an instance saves in the layout, under its type's key.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PanelRecord {
    pub id: Option<InstanceId>,
    /// A name the listener gave the tab. `None` shows the content's title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Legacy toolbar preference retained when round-tripping older layouts.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub toolbar_hidden: bool,
    /// The content's own state.
    #[serde(skip_serializing_if = "serde_json::Value::is_null")]
    pub state: serde_json::Value,
}
impl PanelRecord {
    pub fn read(info: &PanelInfo) -> Self {
        match info {
            PanelInfo::Panel(value) => serde_json::from_value(value.clone()).unwrap_or_default(),
            _ => Self::default(),
        }
    }
    pub fn with_state(state: impl Serialize) -> Self {
        Self {
            state: serde_json::to_value(state).unwrap_or_default(),
            ..Default::default()
        }
    }
    pub fn state<T: serde::de::DeserializeOwned + Default>(&self) -> T {
        serde_json::from_value(self.state.clone()).unwrap_or_default()
    }
    pub fn to_state(&self, key: &str) -> PanelState {
        let mut state = PanelState::new(key);
        state.info = PanelInfo::panel(serde_json::to_value(self).unwrap_or_default());
        state
    }
}

/// What every panel draws and saves. Content holds entities, so the methods
/// take the app rather than a context of their own.
#[allow(unused_variables)]
pub trait PaneContent: 'static {
    /// The content itself, for the workspace to recognise a type it coordinates.
    fn as_any(&self) -> &dyn std::any::Any;
    fn view(&self) -> AnyView;
    fn focus_handle(&self, cx: &App) -> FocusHandle;
    /// The tab label: what the instance shows now.
    fn title(&self, cx: &App) -> SharedString;
    fn icon(&self, ty: &'static PanelType, cx: &App) -> Icon {
        Icon::new(ty.icon)
    }
    fn save(&self, cx: &App) -> serde_json::Value {
        serde_json::Value::Null
    }
    /// The listener brought the panel forward: clicked inside it or chose
    /// its tab.
    fn focused(&self, window: &mut Window, cx: &mut App) {}
    /// A displayed tab in a visible window can run its presentation work.
    /// Analysis and animations can pause between these edges.
    fn set_presented(&self, presented: bool, cx: &mut App) {}
    /// A folder dragged from Locations onto this panel's tab.
    fn accepts_folder(&self) -> bool {
        false
    }
    fn drop_folder(&self, path: PathBuf, cx: &mut App) {}
    /// Subscriptions that notify the pane when its title or content changes,
    /// so its tab follows even while another tab is displayed.
    fn watch(&self, cx: &mut Context<Pane>) -> Vec<Subscription> {
        Vec::new()
    }
}

/// What a pane asks of the workspace that owns the dock.
pub enum PaneEvent {
    /// Something saved with the instance changed.
    Changed,
    Close,
    /// Open the same content in a new instance.
    Duplicate,
}

/// One panel instance in the dock.
pub struct Pane {
    pub ty: &'static PanelType,
    pub id: InstanceId,
    pub name: Option<String>,
    pub toolbar_hidden: bool,
    sidebar_hidden: bool,
    presented: bool,
    pub content: Rc<dyn PaneContent>,
    /// The tab group holding the pane, as the dock last reported it.
    pub group: Option<WeakEntity<TabGroup>>,
    /// The tab's name field while the listener renames it.
    pub renaming: Option<Entity<InputState>>,
    pub tab_focus: FocusHandle,
    pub controls_focus: FocusHandle,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}
impl EventEmitter<PanelEvent> for Pane {}
impl EventEmitter<PaneEvent> for Pane {}

impl Pane {
    /// Whether keyboard focus is in this pane's body or its tab strip controls.
    pub(crate) fn contains_focus(&self, window: &Window, cx: &App) -> bool {
        [&self.focus, &self.tab_focus, &self.controls_focus]
            .into_iter()
            .any(|handle| handle.contains_focused(window, cx))
    }

    pub fn new(
        ty: &'static PanelType,
        id: InstanceId,
        record: &PanelRecord,
        services: &Services,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let content = (ty.build)(services, record, window, cx);
        let focus = cx.focus_handle();
        let mut subscriptions = content.watch(cx);
        let controls_focus = cx.focus_handle();
        let tab_focus = cx.focus_handle();
        for handle in [&focus, &controls_focus, &tab_focus] {
            subscriptions.push(cx.on_focus_in(handle, window, |pane: &mut Self, w, cx| {
                pane.content.focused(w, cx)
            }));
        }
        Self {
            ty,
            id,
            name: record.name.clone().filter(|n| !n.trim().is_empty()),
            sidebar_hidden: false,
            presented: false,
            toolbar_hidden: record.toolbar_hidden,
            content,
            group: None,
            renaming: None,
            tab_focus,
            controls_focus,
            focus,
            _subscriptions: subscriptions,
        }
    }
    pub fn record(&self, cx: &App) -> PanelRecord {
        PanelRecord {
            id: Some(self.id),
            name: self.name.clone(),
            toolbar_hidden: self.toolbar_hidden,
            state: self.content.save(cx),
        }
    }
    /// The tab label: the listener's name for it, or what it shows.
    pub fn label(&self, cx: &App) -> SharedString {
        match &self.name {
            Some(name) => name.clone().into(),
            None => self.content.title(cx),
        }
    }
    pub fn icon(&self, cx: &App) -> Icon {
        self.content.icon(self.ty, cx)
    }
    /// Redraws the tab as well as the content: the group draws the tab of a
    /// pane that is not displayed, and is not told otherwise.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        cx.notify();
        if let Some(group) = self.group.as_ref().and_then(WeakEntity::upgrade) {
            group.update(cx, |_, cx| cx.notify());
        }
    }
    pub fn sync_display(&mut self, hidden: bool, presented: bool, cx: &mut Context<Self>) {
        if self.sidebar_hidden != hidden {
            self.sidebar_hidden = hidden;
            self.refresh(cx);
        }
        if self.presented != presented {
            self.presented = presented;
            self.content.set_presented(presented, cx);
        }
    }
    /// Turns the tab label into a name field.
    pub fn rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.label(cx).to_string();
        let input = cx.new(|cx| InputState::new(window, cx).default_value(current));
        cx.subscribe_in(&input, window, |pane, input, e, window, cx| match e {
            InputEvent::PressEnter { .. } | InputEvent::Blur
                if pane.renaming.as_ref() == Some(input) =>
            {
                let name = input.read(cx).value().trim().to_string();
                pane.renaming = None;
                // A name equal to the content's own title follows the content.
                pane.name =
                    (!name.is_empty() && name != pane.content.title(cx).as_ref()).then_some(name);
                if matches!(e, InputEvent::PressEnter { .. }) {
                    pane.content.focus_handle(cx).focus(window, cx);
                }
                cx.emit(PaneEvent::Changed);
                pane.refresh(cx);
            }
            _ => {}
        })
        .detach();
        input.update(cx, |s, cx| {
            s.focus(window, cx);
            s.select_all(window, cx);
        });
        self.renaming = Some(input);
        self.refresh(cx);
    }
    pub fn cancel_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.renaming.take().is_some() {
            self.content.focus_handle(cx).focus(window, cx);
            self.refresh(cx);
        }
    }
    pub fn reset_name(&mut self, cx: &mut Context<Self>) {
        if self.name.take().is_some() {
            cx.emit(PaneEvent::Changed);
            self.refresh(cx);
        }
    }
    /// Instance operations shared by tab menus.
    pub fn menu(this: &Entity<Self>, menu: PopupMenu, cx: &mut App) -> PopupMenu {
        use gpui_kit::component::menu::PopupMenuItem;
        let pane = this.read(cx);
        let (ty, named) = (pane.ty, pane.name.is_some());
        // Only instances of a kind that can be opened several times need
        // names of their own. A fixed panel such as Queue keeps its title.
        if ty.instances != Instances::Multiple {
            return menu;
        }
        let (b, c, d) = (this.downgrade(), this.downgrade(), this.downgrade());
        menu.item(
            PopupMenuItem::new("Open in New Tab")
                .icon(IconName::Copy)
                .on_click(move |_, _, cx| {
                    let _ = b.update(cx, |_, cx| cx.emit(PaneEvent::Duplicate));
                }),
        )
        .item(
            PopupMenuItem::new("Rename Tab…")
                .icon(IconName::Pencil)
                .on_click(move |_, w, cx| {
                    let _ = c.update(cx, |p, cx| p.rename(w, cx));
                }),
        )
        .when(named, |menu| {
            menu.item(
                PopupMenuItem::new("Reset Tab Name").on_click(move |_, _, cx| {
                    let _ = d.update(cx, |p, cx| p.reset_name(cx));
                }),
            )
        })
    }
    /// Closing a browser tab discards that instance. A fixed panel is only
    /// hidden: the workspace menu shows it again where it was.
    pub fn close_item(this: &Entity<Self>, cx: &App) -> gpui_kit::component::menu::PopupMenuItem {
        let ty = this.read(cx).ty;
        let label = match ty.instances {
            Instances::Multiple => "Close Tab".to_string(),
            Instances::Single => format!("Hide {}", ty.title),
        };
        let this = this.downgrade();
        gpui_kit::component::menu::PopupMenuItem::new(label)
            .icon(IconName::X)
            .on_click(move |_, _, cx| {
                let _ = this.update(cx, |_, cx| cx.emit(PaneEvent::Close));
            })
    }
}

impl Focusable for Pane {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.content.focus_handle(cx)
    }
}

impl BasePanel for Pane {
    fn visible(&self, _: &App) -> bool {
        !self.sidebar_hidden
    }
    fn panel_name(&self) -> &'static str {
        self.ty.key
    }
    fn zoomable(&self, _: &App) -> bool {
        false
    }
    fn on_added_to(&mut self, group: WeakEntity<TabGroup>, _: &mut Window, _: &mut Context<Self>) {
        self.group = Some(group);
    }
    fn on_removed(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.group = None;
        self.sync_display(self.sidebar_hidden, false, cx);
    }
    fn dump(&self, cx: &App) -> PanelState {
        self.record(cx).to_state(self.ty.key)
    }
}

impl Render for Pane {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .track_focus(&self.focus)
            // Seen before the content handles the press, so a click anywhere in
            // the panel makes it the one the workspace addresses.
            .capture_any_mouse_down(
                cx.listener(|pane, _, w, cx| pane.content.clone().focused(w, cx)),
            )
            .size_full()
            .relative()
            .overflow_hidden()
            .child(div().flex_1().min_h_0().child(self.content.view()))
    }
}

/// Where navigation goes: a folder on disk or a library view.
#[derive(Clone, Debug, PartialEq)]
pub enum Destination {
    /// A folder, shown under the Place `root` when one is named.
    Folder {
        path: PathBuf,
        root: Option<PathBuf>,
    },
    View(Location),
}
impl Destination {
    /// The same explicit browser actions for every navigation destination.
    pub fn open_items(self, menu: PopupMenu, router: Entity<Router>) -> PopupMenu {
        use gpui_kit::component::menu::PopupMenuItem;
        let tab = router.clone();
        let destination = self.clone();
        menu.item(
            PopupMenuItem::new("Open in New Tab").on_click(move |_, _, cx| {
                tab.update(cx, |_, cx| cx.emit(RouterEvent::Open(destination.clone())));
            }),
        )
        .item(PopupMenuItem::new("Open Beside").on_click(move |_, _, cx| {
            router.update(cx, |_, cx| cx.emit(RouterEvent::Beside(self.clone())));
        }))
    }

    pub fn apply(self, browser: &mut Browser, window: &mut Window, cx: &mut Context<Browser>) {
        match self {
            Self::Folder {
                path,
                root: Some(root),
            } => browser.open_in(path, root, window, cx),
            Self::Folder { path, root: None } => browser.open(path, window, cx),
            Self::View(location) => browser.show(location, window, cx),
        }
    }
}

/// What new Music Browsers start with: the column layouts and view settings
/// most recently changed in any browser.
#[derive(Clone)]
pub struct Template {
    pub layouts: Layouts,
    pub prefs: ViewPrefs,
}

pub enum RouterEvent {
    /// Open a destination in a new Music Browser.
    Open(Destination),
    Beside(Destination),
    /// Show the active browser where it sits in the dock.
    Reveal,
}

/// The active Music Browser, which navigation panels, the address bar and
/// the search field address. It is the browser that last had focus.
pub struct Router {
    active: Option<WeakEntity<Browser>>,
    pub template: Template,
}
impl EventEmitter<RouterEvent> for Router {}
impl Router {
    pub fn new(template: Template) -> Self {
        Self {
            active: None,
            template,
        }
    }
    pub fn active(&self) -> Option<Entity<Browser>> {
        self.active.as_ref().and_then(WeakEntity::upgrade)
    }
    pub fn set_active(&mut self, browser: Option<&Entity<Browser>>, cx: &mut Context<Self>) {
        if self.active.as_ref().map(|b| b.entity_id()) == browser.map(|b| b.entity_id()) {
            return;
        }
        if let Some(previous) = self.active() {
            previous.update(cx, |b, cx| b.set_active(false, cx));
        }
        self.active = browser.map(Entity::downgrade);
        if let Some(browser) = browser {
            browser.update(cx, |b, cx| b.set_active(true, cx));
        }
        cx.notify();
    }
    /// Ordinary navigation addresses an existing browser. Only the new-tab
    /// action asks the dock to create an instance.
    pub fn open(
        router: &Entity<Self>,
        destination: Destination,
        new_tab: bool,
        window: &mut Window,
        cx: &mut App,
    ) {
        if !new_tab && let Some(browser) = router.read(cx).active() {
            browser.update(cx, |b, cx| destination.apply(b, window, cx));
            router.update(cx, |_, cx| cx.emit(RouterEvent::Reveal));
        } else {
            router.update(cx, |_, cx| cx.emit(RouterEvent::Open(destination)));
        }
    }
}

/// The state every panel shares, whatever its type or placement.
#[derive(Clone)]
pub struct Services {
    pub player: Entity<Player>,
    pub catalog: Entity<Catalog>,
    pub locations: Entity<locations::Locations>,
    pub router: Entity<Router>,
}

#[cfg(test)]
mod tests {
    use super::{InstanceId, PanelRecord, TYPES};
    use gpui_kit::component::dock::PanelInfo;

    #[test]
    fn every_type_has_a_distinct_persisted_key() {
        let mut keys: Vec<_> = TYPES.iter().map(|t| t.key).collect();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), TYPES.len());
    }

    #[test]
    fn records_round_trip_through_the_dock_state() {
        let record = PanelRecord {
            id: Some(InstanceId(42)),
            name: Some("Bass".into()),
            toolbar_hidden: true,
            state: serde_json::json!({"zoom": 4}),
        };
        let state = record.to_state("disqro.browser");
        assert_eq!(state.panel_name, "disqro.browser");
        assert_eq!(PanelRecord::read(&state.info), record);
    }

    #[test]
    fn a_minimal_record_writes_only_its_identity() {
        let record = PanelRecord {
            id: Some(InstanceId(7)),
            ..Default::default()
        };
        assert_eq!(
            serde_json::to_value(&record).unwrap(),
            serde_json::json!({"id": 7})
        );
        // Unknown or damaged state falls back to a fresh instance.
        assert_eq!(
            PanelRecord::read(&PanelInfo::panel(serde_json::json!("garbage"))),
            PanelRecord::default()
        );
    }

    #[test]
    fn fresh_identities_avoid_those_taken() {
        let taken = [InstanceId(1), InstanceId(2)];
        for _ in 0..100 {
            let id = InstanceId::fresh(|id| taken.contains(&id));
            assert!(!taken.contains(&id));
            // Kept below 2^53 so the JSON value is exact in any reader.
            assert!(id.0 < 1 << 53);
        }
    }
}
