//! The patched GPUI defers OS termination requests to an app handler.
//!
//! macOS asks `applicationShouldTerminate:` for Dock Quit, logout, restart and
//! `App::quit`. Without a handler the platform terminates at once, as upstream
//! GPUI does; with one, it waits for exactly one `App::reply_to_quit`. The test
//! platform records each answer so headless tests can drive the request.
use gpui::TestAppContext;
use std::{cell::Cell, rc::Rc};

#[gpui::test]
fn termination_without_handler_is_immediate(cx: &mut TestAppContext) {
    assert!(!cx.simulate_should_quit());
    assert!(!cx.quit_request_pending());
    cx.update(|cx| cx.reply_to_quit(false));
    assert!(cx.quit_replies().is_empty(), "no request was pending");
}

#[gpui::test]
fn handler_defers_termination_until_its_reply(cx: &mut TestAppContext) {
    let asked = Rc::new(Cell::new(0));
    cx.update(|cx| {
        let asked = asked.clone();
        cx.on_should_quit(move |_| asked.set(asked.get() + 1));
    });

    // Cancel: the request stays pending until the later reply.
    assert!(cx.simulate_should_quit());
    assert_eq!(asked.get(), 1);
    assert!(cx.quit_request_pending());
    cx.update(|cx| cx.reply_to_quit(false));
    assert!(!cx.quit_request_pending());
    cx.update(|cx| cx.reply_to_quit(true));
    assert_eq!(
        cx.quit_replies(),
        [false],
        "only a pending request is answered"
    );

    // The handler survives its call and answers the next request.
    assert!(cx.simulate_should_quit());
    assert_eq!(asked.get(), 2);
    cx.update(|cx| cx.reply_to_quit(true));
    assert_eq!(cx.quit_replies(), [false, true]);
}

#[gpui::test]
fn handler_may_reply_before_returning_and_be_replaced(cx: &mut TestAppContext) {
    cx.update(|cx| cx.on_should_quit(|cx| cx.reply_to_quit(true)));
    assert!(cx.simulate_should_quit());
    assert!(!cx.quit_request_pending());

    // A handler installed during the call replaces the running one.
    cx.update(|cx| {
        cx.on_should_quit(|cx| {
            cx.on_should_quit(|cx| cx.reply_to_quit(true));
            cx.reply_to_quit(false);
        })
    });
    assert!(cx.simulate_should_quit());
    assert!(cx.simulate_should_quit());
    assert_eq!(cx.quit_replies(), [true, false, true]);
}
