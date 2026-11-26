use super::*;
use crate::panels::Template;
use std::{cell::Cell, rc::Rc};

type RevealResult = anyhow::Result<Option<Arc<Track>>>;

fn delay_reveal(
    browser: &Entity<Browser>,
    router: &Entity<Router>,
    w: &mut Window,
    cx: &mut App,
) -> (async_channel::Sender<RevealResult>, Task<()>) {
    let (result, lookup) = async_channel::bounded(1);
    let task = browser.update(cx, |b, cx| {
        let request = b.begin_navigation(cx);
        b.resolve_recording_reveal(
            request,
            async move { lookup.recv().await.unwrap() },
            router.clone(),
            w,
            cx,
        )
    });
    (result, task)
}

#[::core::prelude::v1::test]
#[ignore = "requires a native Windows window"]
fn delayed_ranked_reveal_respects_browser_navigation_and_lifetime() {
    let music = tempfile::tempdir().unwrap();
    let first = Arc::new(Track::file(music.path().join("First").join("One.wav")));
    let second_folder = music.path().join("Second");
    let second = Arc::new(Track::file(second_folder.join("Two.wav")));
    std::fs::create_dir_all(&second_folder).unwrap();
    let completed = Rc::new(Cell::new(false));
    let finished = completed.clone();
    gpui_kit::application()
        .with_assets(crate::assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            crate::theme::install(crate::theme::Appearance::Dark, cx);
            let (window, _) =
                gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| cx.new(|_| EmptyView))
                    .unwrap();
            let (router, a, b) = window
                .update(cx, |_, w, cx| {
                    let catalog = cx.new(|cx| Catalog::open(None, cx));
                    let folders = cx.new(Folders::new);
                    let player =
                        cx.new(|cx| Player::new(0., false, false, None, Default::default(), cx));
                    let mut new_browser = || {
                        cx.new(|cx| {
                            Browser::new(
                                player.clone(),
                                catalog.clone(),
                                folders.clone(),
                                Layouts::default(),
                                ViewPrefs::default(),
                                w,
                                cx,
                            )
                        })
                    };
                    let (a, b) = (new_browser(), new_browser());
                    let router = cx.new(|cx| {
                        let mut router = Router::new(Template {
                            layouts: Layouts::default(),
                            prefs: ViewPrefs::default(),
                        });
                        router.set_active(Some(&a), cx);
                        router
                    });
                    (router, a, b)
                })
                .unwrap();
            let reveals = Rc::new(Cell::new(0));
            let observed = reveals.clone();
            let events = cx.subscribe(&router, move |_, event, _| {
                assert!(matches!(event, RouterEvent::Reveal(_)));
                observed.set(observed.get() + 1);
            });

            cx.spawn(async move |cx| {
                let _events = events;
                let (old, task) = window
                    .update(cx, |_, w, cx| {
                        let pending = delay_reveal(&a, &router, w, cx);
                        a.update(cx, |b, cx| b.show(Location::Liked, w, cx));
                        pending
                    })
                    .unwrap();
                old.try_send(Ok(Some(first.clone()))).unwrap();
                task.await;
                window
                    .update(cx, |_, w, cx| {
                        assert_eq!(a.read(cx).location, Location::Liked);
                        assert!(w.notifications(cx).is_empty());
                        assert_eq!(reveals.get(), 0);
                    })
                    .unwrap();

                let (old, task) = window
                    .update(cx, |_, w, cx| {
                        let pending = delay_reveal(&a, &router, w, cx);
                        router.update(cx, |r, cx| r.set_active(Some(&b), cx));
                        pending
                    })
                    .unwrap();
                old.try_send(Err(anyhow::anyhow!("Obsolete lookup error")))
                    .unwrap();
                task.await;
                window
                    .update(cx, |_, w, cx| {
                        assert_eq!(router.read(cx).active(), Some(b.clone()));
                        assert_eq!(a.read(cx).location, Location::Liked);
                        assert_eq!(b.read(cx).location, Location::Folder(PathBuf::new()));
                        assert!(w.notifications(cx).is_empty());
                        assert_eq!(reveals.get(), 0);
                    })
                    .unwrap();

                let ((older, older_task), (newer, newer_task)) = window
                    .update(cx, |_, w, cx| {
                        router.update(cx, |r, cx| r.set_active(Some(&a), cx));
                        (
                            delay_reveal(&a, &router, w, cx),
                            delay_reveal(&a, &router, w, cx),
                        )
                    })
                    .unwrap();
                newer.try_send(Ok(Some(second))).unwrap();
                newer_task.await;
                older.try_send(Ok(Some(first.clone()))).unwrap();
                older_task.await;
                window
                    .update(cx, |_, w, cx| {
                        assert_eq!(a.read(cx).location, Location::Folder(second_folder));
                        assert!(w.notifications(cx).is_empty());
                        assert_eq!(reveals.get(), 1);
                    })
                    .unwrap();

                let (old, task) = window
                    .update(cx, |_, w, cx| {
                        let pending = delay_reveal(&a, &router, w, cx);
                        router.update(cx, |r, cx| r.set_active(Some(&b), cx));
                        pending
                    })
                    .unwrap();
                let closed = a.downgrade();
                drop(a);
                old.try_send(Ok(Some(first))).unwrap();
                task.await;
                window
                    .update(cx, |_, w, cx| {
                        assert!(closed.upgrade().is_none());
                        assert_eq!(router.read(cx).active(), Some(b.clone()));
                        assert_eq!(b.read(cx).location, Location::Folder(PathBuf::new()));
                        assert!(w.notifications(cx).is_empty());
                        assert_eq!(reveals.get(), 1);
                    })
                    .unwrap();

                let (unavailable, task) = window
                    .update(cx, |_, w, cx| delay_reveal(&b, &router, w, cx))
                    .unwrap();
                unavailable.try_send(Ok(None)).unwrap();
                task.await;
                window
                    .update(cx, |_, w, cx| {
                        assert_eq!(b.read(cx).location, Location::Folder(PathBuf::new()));
                        assert_eq!(w.notifications(cx).len(), 1);
                        assert_eq!(reveals.get(), 1);
                    })
                    .unwrap();
                window
                    .update(cx, |_, _, cx| {
                        b.update(cx, |browser, cx| {
                            browser.pending = Some(Pending {
                                bookmark: Bookmark::default(),
                                target: Some(RowKey::Key(7)),
                                recording: true,
                            });
                            tree::tests::finish_filtered_arrival(browser, cx);
                        });
                    })
                    .unwrap();
                finished.set(true);
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
    assert!(completed.get());
}
