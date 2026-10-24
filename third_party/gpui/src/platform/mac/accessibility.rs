//! AppKit bridge: cross-thread objects contain data only; window/GPUI owners stay on UI.
use super::*;
use crate::platform::accessibility_bridge::{Bridge, Registry, WindowKey};
use std::{cell::RefCell, collections::VecDeque};

pub(super) const DATA_IVAR: &str = "semanticBridge";
static mut CONTROL_CLASS: *const Class = ptr::null();

#[derive(Clone)]
struct SnapshotNode {
    node: crate::AccessibilityNode,
    screen: [f64; 4],
}
pub(super) struct Snapshot {
    nodes: Vec<SnapshotNode>,
    refused: bool,
}
pub(super) struct Action {
    request: crate::AccessibilityRequest,
    owner: Weak<()>,
    inventory: Option<Vec<SharedString>>,
}
pub(super) type NativeBridge = Bridge<Snapshot, Action>;
struct Element {
    id: crate::AccessibilityId,
    owner: Weak<()>,
    object: id,
}
impl Drop for Element {
    fn drop(&mut self) {
        // Element cache is exclusively UI-owned. Its native object contains data only.
        unsafe {
            let _: () = msg_send![self.object, release];
        }
    }
}
struct Entry {
    bridge: Arc<NativeBridge>,
    view: id, // Borrowed real view, usable only while this UI registry entry remains live.
    elements: Vec<Rc<Element>>,
    callback: Option<Box<dyn FnMut(crate::AccessibilityRequest) -> bool>>,
}
thread_local! { static WINDOWS: RefCell<Registry<Entry>> = RefCell::new(Registry::default()); }

fn on_ui() -> bool {
    unsafe {
        let main: BOOL = msg_send![class!(NSThread), isMainThread];
        main == YES
    }
}

pub(super) unsafe fn initialize_class() {
    unsafe {
        CONTROL_CLASS = build_class();
    }
}

pub(super) fn register(window: u64, view: id) -> Option<Arc<NativeBridge>> {
    fn send_sync<T: Send + Sync>() {}
    send_sync::<NativeBridge>();
    send_sync::<ElementData>();
    if !on_ui() {
        return None;
    }
    WINDOWS.with(|registry| {
        registry.borrow_mut().register(window, |bridge| Entry {
            bridge,
            view,
            elements: Vec::new(),
            callback: None,
        })
    })
}

pub(super) unsafe fn attach(object: id, bridge: &Arc<NativeBridge>) {
    unsafe {
        (*object).set_ivar(
            DATA_IVAR,
            Box::into_raw(Box::new(bridge.clone())) as *mut c_void,
        );
    }
}
fn object_bridge(this: &Object) -> Option<&Arc<NativeBridge>> {
    let raw = unsafe { *this.get_ivar::<*mut c_void>(DATA_IVAR) };
    unsafe { (raw as *const Arc<NativeBridge>).as_ref() }
}
pub(super) fn revoke_object(this: &Object) {
    if let Some(bridge) = object_bridge(this) {
        revoke(bridge);
    }
}
// Use the Objective-C runtime setter rather than fabricating &mut Object from &Object.
// This is a non-object (void*) ivar: the runtime does not retain/release the boxed Rust owner.
#[link(name = "objc")]
unsafe extern "C" {
    fn object_getInstanceVariable(
        object: *const Object,
        name: *const std::ffi::c_char,
        value: *mut *mut c_void,
    ) -> *mut c_void;
    fn object_setInstanceVariable(
        object: *const Object,
        name: *const std::ffi::c_char,
        value: *mut c_void,
    ) -> *mut c_void;
}

/// Transfer an exclusively owned native ivar into Rust, making reentrant readers see null first.
/// The caller owns native teardown; a live ObjC receiver cannot concurrently be deallocated.
unsafe fn take_owned_ivar<T>(this: &Object, name: &CStr) -> Option<Box<T>> {
    unsafe {
        let mut raw = ptr::null_mut();
        if object_getInstanceVariable(this, name.as_ptr(), &mut raw).is_null() || raw.is_null() {
            return None;
        }
        // A missing ivar refuses ownership transfer; never consume a pointer we failed to clear.
        if object_setInstanceVariable(this, name.as_ptr(), ptr::null_mut()).is_null() {
            return None;
        }
        Some(Box::from_raw(raw.cast::<T>()))
    }
}

pub(super) unsafe fn detach(this: &Object) {
    unsafe {
        if let Some(bridge) = take_owned_ivar::<Arc<NativeBridge>>(this, c"semanticBridge") {
            // The owning ivar is already null before revoke releases reentrant callbacks/cache.
            revoke(&bridge);
            drop(bridge);
        }
    }
}

pub(super) fn revoke(bridge: &Arc<NativeBridge>) {
    let newly_revoked = bridge.revoke(); // Unavailable/empty before any UI-owner release.
    if on_ui() {
        let retired = WINDOWS.with(|registry| registry.borrow_mut().remove(bridge.key));
        drop(retired); // Native release and callback drop may reenter, with no registry borrow.
    } else if newly_revoked {
        schedule(bridge.key); // ID only; cleanup must happen on UI, never on the query thread.
    }
}
pub(super) fn set_callback(
    bridge: &Arc<NativeBridge>,
    callback: Box<dyn FnMut(crate::AccessibilityRequest) -> bool>,
) {
    if !on_ui() {
        return;
    }
    let mut incoming = Some(callback);
    let retired = WINDOWS.with(|registry| {
        let mut registry = registry.borrow_mut();
        if let Some(entry) = registry.get_mut(bridge.key) {
            std::mem::replace(&mut entry.callback, incoming.take())
        } else {
            None
        }
    });
    drop(retired);
    drop(incoming); // Missing/revoked registration also drops the callback outside the borrow.
}

pub(super) fn publish(
    bridge: &Arc<NativeBridge>,
    nodes: Vec<crate::AccessibilityNode>,
    frames: Vec<[f64; 4]>,
    refused: bool,
) {
    if !on_ui() || bridge.is_revoked() {
        return;
    }
    let mut ids = std::collections::HashSet::new();
    let mut text_bytes = 0usize;
    let invalid = refused
        || nodes.len() > crate::MAX_ACCESSIBILITY_NODES
        || nodes.len() != frames.len()
        || nodes.iter().any(|node| {
            let descriptor = &node.descriptor;
            if !descriptor.is_valid() || !ids.insert(descriptor.id) {
                return true;
            }
            let value_bytes = match &descriptor.value {
                crate::AccessibilityValue::Number { text, .. } => text.len(),
                crate::AccessibilityValue::Button | crate::AccessibilityValue::Toggle(_) => 0,
                crate::AccessibilityValue::Choice { options, .. } => {
                    options.iter().map(|text| text.len()).sum()
                }
            };
            let bytes = descriptor.name.len()
                + descriptor.status.as_ref().map_or(0, |text| text.len())
                + value_bytes;
            if bytes > crate::MAX_ACCESSIBILITY_SNAPSHOT_BYTES.saturating_sub(text_bytes) {
                return true;
            }
            text_bytes += bytes;
            false
        })
        || frames.iter().any(|frame| {
            frame.iter().any(|value| !value.is_finite()) || frame[2] <= 0. || frame[3] <= 0.
        });
    let previous = bridge.snapshot();
    let mut snapshot = Snapshot {
        nodes: Vec::new(),
        refused: invalid,
    };
    if !invalid {
        snapshot.nodes = nodes
            .into_iter()
            .zip(frames)
            .map(|(node, screen)| SnapshotNode { node, screen })
            .collect();
    }
    // Take the entry, so every native allocation/release can reenter without a RefCell borrow.
    let entry = WINDOWS.with(|registry| registry.borrow_mut().remove(bridge.key));
    let Some(mut entry) = entry else {
        return;
    };
    let mut retained = Vec::with_capacity(snapshot.nodes.len());
    for element in std::mem::take(&mut entry.elements) {
        if snapshot.nodes.iter().any(|node| {
            node.node.descriptor.id == element.id
                && Weak::ptr_eq(&node.node.descriptor.lifetime, &element.owner)
        }) {
            retained.push(element);
        } else {
            drop(element);
        }
    }
    let mut failed = false;
    for node in &snapshot.nodes {
        let descriptor = &node.node.descriptor;
        if retained.iter().any(|element| element.id == descriptor.id) {
            continue;
        }
        let object: id = unsafe {
            if CONTROL_CLASS.is_null() {
                nil
            } else {
                let allocated: id = msg_send![CONTROL_CLASS, alloc];
                if allocated.is_null() {
                    nil
                } else {
                    msg_send![allocated, init]
                }
            }
        };
        if object.is_null() {
            failed = true;
            break;
        }
        let data = Box::new(ElementData {
            bridge: bridge.clone(),
            id: descriptor.id,
            owner: descriptor.lifetime.clone(),
        });
        unsafe {
            (*object).set_ivar("semanticState", Box::into_raw(data) as *mut c_void);
        }
        retained.push(Rc::new(Element {
            id: descriptor.id,
            owner: descriptor.lifetime.clone(),
            object,
        }));
    }
    if failed {
        snapshot.nodes.clear();
        snapshot.refused = true;
        retained.clear();
    }
    entry.elements = retained;
    // An allocation/retirement hook may have closed this window. Never restore its registry entry.
    if bridge.is_revoked() {
        drop(entry);
        return;
    }
    let notifications: Vec<(Rc<Element>, bool, bool)> = snapshot
        .nodes
        .iter()
        .filter_map(|current| {
            let current = &current.node.descriptor;
            let old = previous.as_ref()?.nodes.iter().find(|old| {
                old.node.descriptor.id == current.id
                    && Weak::ptr_eq(&old.node.descriptor.lifetime, &current.lifetime)
            })?;
            let old = &old.node.descriptor;
            let changed = old.revision != current.revision && old.value != current.value;
            let focus = !old.focused && current.focused;
            let element = entry
                .elements
                .iter()
                .find(|element| element.id == current.id)?;
            if changed || focus {
                Some((element.clone(), changed, focus))
            } else {
                None
            }
        })
        .collect();
    drop(bridge.publish(snapshot));
    let displaced = WINDOWS.with(|registry| registry.borrow_mut().restore(bridge.key, entry));
    drop(displaced);
    for (element, changed, focus) in notifications {
        unsafe {
            if !bridge.is_revoked() {
                if changed {
                    NSAccessibilityPostNotification(
                        element.object,
                        NSAccessibilityValueChangedNotification,
                    );
                }
                if focus {
                    NSAccessibilityPostNotification(
                        element.object,
                        NSAccessibilityFocusedUIElementChangedNotification,
                    );
                }
            }
        }
    }
}

// No MacWindowState, NSWindow/View, callback, Rc or executor is reachable from a native element.
struct ElementData {
    bridge: Arc<NativeBridge>,
    id: crate::AccessibilityId,
    owner: Weak<()>,
}
fn semantic_state(this: &Object) -> Option<&ElementData> {
    let raw = unsafe { *this.get_ivar::<*mut c_void>("semanticState") };
    unsafe { (raw as *const ElementData).as_ref() }
}
fn current_node(data: &ElementData) -> Option<SnapshotNode> {
    let snapshot = data.bridge.snapshot()?;
    let node = snapshot
        .nodes
        .iter()
        .find(|node| {
            node.node.descriptor.id == data.id
                && Weak::ptr_eq(&node.node.descriptor.lifetime, &data.owner)
                && node.node.descriptor.is_valid()
        })?
        .clone();
    if data.bridge.is_revoked() {
        None
    } else {
        Some(node)
    }
}
fn semantic_node(this: &Object) -> Option<crate::AccessibilityNode> {
    current_node(semantic_state(this)?).map(|node| node.node)
}
extern "C" fn accessibility_dealloc(this: &Object, _: Sel) {
    unsafe {
        // Clear before dropping the boxed data: nested queries/teardown must see unavailable.
        drop(take_owned_ivar::<ElementData>(this, c"semanticState"));
        let _: () = msg_send![super(this, class!(NSAccessibilityElement)), dealloc];
    }
}
pub(super) extern "C" fn children(this: &Object, _: Sel) -> id {
    if !on_ui() {
        return nil;
    } // Object-tree access is UI-only, never synchronous dispatch.
    let Some(bridge) = object_bridge(this) else {
        return nil;
    };
    if bridge.is_revoked() {
        return nil;
    }
    // Retain a temporary cache copy while on UI, then release the registry borrow before ObjC calls.
    let elements: Vec<Rc<Element>> = WINDOWS.with(|registry| {
        registry
            .borrow()
            .get(bridge.key)
            .map_or_else(Vec::new, |entry| {
                entry
                    .elements
                    .iter()
                    .filter(|element| element.owner.strong_count() > 0)
                    .cloned()
                    .collect()
            })
    });
    unsafe {
        let array: id = msg_send![class!(NSMutableArray), array];
        for element in elements {
            if !bridge.is_revoked() {
                let _: () = msg_send![array, addObject: element.object];
            }
        }
        array
    }
}

pub(super) extern "C" fn root_label(this: &Object, _: Sel) -> id {
    let snapshot = object_bridge(this).and_then(|bridge| bridge.snapshot());
    unsafe {
        ns_string(if snapshot.is_some_and(|snapshot| !snapshot.refused) {
            "controls"
        } else {
            "controls unavailable"
        })
    }
}
extern "C" fn accessibility_frame(this: &Object, _: Sel) -> NSRect {
    let frame = semantic_state(this)
        .and_then(current_node)
        .map_or([0.; 4], |node| node.screen);
    NSRect::new(
        NSPoint::new(frame[0], frame[1]),
        NSSize::new(frame[2], frame[3]),
    )
}
extern "C" fn accessibility_parent(this: &Object, _: Sel) -> id {
    if !on_ui() {
        return nil;
    }
    let Some(data) = semantic_state(this) else {
        return nil;
    };
    if current_node(data).is_none() {
        return nil;
    }
    let parent = WINDOWS.with(|registry| {
        registry
            .borrow()
            .get(data.bridge.key)
            .map_or(nil, |entry| entry.view)
    });
    if parent.is_null() {
        nil
    } else {
        unsafe {
            let retained: id = msg_send![parent, retain];
            msg_send![retained, autorelease]
        }
    }
}
extern "C" fn accessibility_enabled(this: &Object, _: Sel) -> BOOL {
    semantic_node(this)
        .is_some_and(|node| node.descriptor.enabled)
        .to_objc()
}
extern "C" fn accessibility_focused(this: &Object, _: Sel) -> BOOL {
    semantic_node(this)
        .is_some_and(|node| node.descriptor.focused)
        .to_objc()
}
extern "C" fn accessibility_hidden(this: &Object, _: Sel) -> BOOL {
    semantic_node(this).is_none().to_objc()
}

// Exact owner/revision and immutable choice inventory are revalidated after the handoff.
fn permits(snapshot: &Snapshot, action: &Action) -> bool {
    snapshot.nodes.iter().any(|node| {
        let node = &node.node.descriptor;
        node.id == action.request.id
            && node.revision == action.request.revision
            && Weak::ptr_eq(&node.lifetime, &action.owner)
            && node.permits(&action.request.action)
            && match (&node.value, &action.inventory) {
                (crate::AccessibilityValue::Choice { options, .. }, Some(inventory)) => {
                    options == inventory
                }
                (crate::AccessibilityValue::Choice { .. }, None) | (_, Some(_)) => false,
                (_, None) => true,
            }
    })
}
fn semantic_action(this: &Object, action: crate::AccessibilityAction) -> BOOL {
    let Some(data) = semantic_state(this) else {
        return NO;
    };
    let Some(node) = current_node(data) else {
        return NO;
    };
    submit(data, &node.node.descriptor, action)
}
fn submit(
    data: &ElementData,
    descriptor: &crate::AccessibilityDescriptor,
    action: crate::AccessibilityAction,
) -> BOOL {
    let inventory = match &descriptor.value {
        crate::AccessibilityValue::Choice { options, .. } => Some(options.clone()),
        _ => None,
    };
    let action = Action {
        request: crate::AccessibilityRequest {
            id: data.id,
            revision: descriptor.revision,
            action,
        },
        owner: data.owner.clone(),
        inventory,
    };
    match data.bridge.enqueue(action, permits) {
        Some(wake) => {
            if wake {
                schedule(data.bridge.key);
            }
            YES
        }
        None => NO,
    }
}
fn schedule(key: WindowKey) {
    // Payload contains ONLY checked IDs. No closures, App/GPUI refs or weak native-window refs.
    let payload = Box::into_raw(Box::new(key));
    unsafe {
        dispatch_async_f(
            dispatch_get_main_queue(),
            payload as *mut c_void,
            Some(drain),
        );
    }
}
extern "C" fn drain(payload: *mut c_void) {
    let key = unsafe { *Box::from_raw(payload as *mut WindowKey) };
    if !on_ui() {
        return;
    }
    let bridge =
        WINDOWS.with(|registry| registry.borrow().get(key).map(|entry| entry.bridge.clone()));
    let Some(bridge) = bridge else {
        return;
    };
    if bridge.is_revoked() {
        revoke(&bridge);
        return;
    }
    let requests: VecDeque<Action> = bridge.drain();
    let callback = WINDOWS.with(|registry| {
        registry
            .borrow_mut()
            .get_mut(key)
            .and_then(|entry| entry.callback.take())
    });
    let Some(mut callback) = callback else {
        return;
    };
    for action in requests {
        if bridge
            .snapshot()
            .is_some_and(|snapshot| permits(&snapshot, &action))
            && !bridge.is_revoked()
        {
            // Revalidates actual GPUI/current project guards; no registry/snapshot borrow held.
            let _canonical_accepted = callback(action.request);
        }
    }
    let mut callback = Some(callback);
    WINDOWS.with(|registry| {
        if !bridge.is_revoked()
            && let Some(entry) = registry.borrow_mut().get_mut(key)
        {
            // A reentrant replacement has priority; never overwrite it or resurrect a closed owner.
            if entry.callback.is_none() {
                entry.callback = callback.take();
            }
        }
    });
    drop(callback); // UI-only, after all registry borrows end.
}

unsafe fn build_class() -> *const Class {
    unsafe {
        let Some(superclass) = Class::get("NSAccessibilityElement") else {
            return ptr::null();
        };
        let Some(mut decl) = ClassDecl::new("GPUISemanticControl", superclass) else {
            return ptr::null();
        };
        decl.add_ivar::<*mut c_void>("semanticState");
        decl.add_method(
            sel!(dealloc),
            accessibility_dealloc as extern "C" fn(&Object, Sel),
        );
        decl.add_method(
            sel!(isAccessibilityHidden),
            accessibility_hidden as extern "C" fn(&Object, Sel) -> BOOL,
        );
        decl.add_method(
            sel!(isAccessibilityElement),
            accessibility_is_element as extern "C" fn(&Object, Sel) -> BOOL,
        );
        decl.add_method(
            sel!(accessibilityRole),
            accessibility_role as extern "C" fn(&Object, Sel) -> id,
        );
        decl.add_method(
            sel!(accessibilityHelp),
            accessibility_help as extern "C" fn(&Object, Sel) -> id,
        );
        decl.add_method(
            sel!(accessibilityLabel),
            accessibility_label as extern "C" fn(&Object, Sel) -> id,
        );
        decl.add_method(
            sel!(accessibilityValue),
            accessibility_value as extern "C" fn(&Object, Sel) -> id,
        );
        decl.add_method(
            sel!(accessibilityValueDescription),
            accessibility_value_description as extern "C" fn(&Object, Sel) -> id,
        );
        decl.add_method(
            sel!(accessibilityMinValue),
            accessibility_min as extern "C" fn(&Object, Sel) -> id,
        );
        decl.add_method(
            sel!(accessibilityMaxValue),
            accessibility_max as extern "C" fn(&Object, Sel) -> id,
        );
        decl.add_method(
            sel!(accessibilityAllowedValues),
            accessibility_allowed_values as extern "C" fn(&Object, Sel) -> id,
        );
        decl.add_method(
            sel!(accessibilityFrame),
            accessibility_frame as extern "C" fn(&Object, Sel) -> NSRect,
        );
        decl.add_method(
            sel!(accessibilityParent),
            accessibility_parent as extern "C" fn(&Object, Sel) -> id,
        );
        decl.add_method(
            sel!(isAccessibilityEnabled),
            accessibility_enabled as extern "C" fn(&Object, Sel) -> BOOL,
        );
        decl.add_method(
            sel!(isAccessibilityFocused),
            accessibility_focused as extern "C" fn(&Object, Sel) -> BOOL,
        );
        decl.add_method(
            sel!(setAccessibilityFocused:),
            accessibility_set_focus as extern "C" fn(&Object, Sel, BOOL),
        );
        decl.add_method(
            sel!(setAccessibilityValue:),
            accessibility_set_value as extern "C" fn(&Object, Sel, id),
        );
        decl.add_method(
            sel!(accessibilityPerformPress),
            accessibility_press as extern "C" fn(&Object, Sel) -> BOOL,
        );
        decl.add_method(
            sel!(accessibilityPerformIncrement),
            accessibility_increment as extern "C" fn(&Object, Sel) -> BOOL,
        );
        decl.add_method(
            sel!(accessibilityPerformDecrement),
            accessibility_decrement as extern "C" fn(&Object, Sel) -> BOOL,
        );
        decl.register()
    }
}

extern "C" fn accessibility_is_element(this: &Object, _: Sel) -> BOOL {
    if semantic_node(this).is_some() {
        YES
    } else {
        NO
    }
}
extern "C" fn accessibility_role(this: &Object, _: Sel) -> id {
    let Some(node) = semantic_node(this) else {
        return nil;
    };
    unsafe {
        ns_string(match node.descriptor.value {
            crate::AccessibilityValue::Button => "AXButton",
            crate::AccessibilityValue::Number { .. } => "AXSlider",
            crate::AccessibilityValue::Toggle(_) => "AXCheckBox",
            crate::AccessibilityValue::Choice { .. } => "AXPopUpButton",
        })
    }
}
extern "C" fn accessibility_label(this: &Object, _: Sel) -> id {
    semantic_node(this).map_or(nil, |node| unsafe { ns_string(&node.descriptor.name) })
}
extern "C" fn accessibility_value(this: &Object, _: Sel) -> id {
    let Some(node) = semantic_node(this) else {
        return nil;
    };
    unsafe {
        match node.descriptor.value {
            crate::AccessibilityValue::Button => nil,
            crate::AccessibilityValue::Number { value, .. } => {
                msg_send![class!(NSNumber), numberWithDouble: value]
            }
            crate::AccessibilityValue::Toggle(value) => {
                msg_send![class!(NSNumber), numberWithBool: if value { YES } else { NO }]
            }
            crate::AccessibilityValue::Choice { selected, options } => {
                options.get(selected).map_or(nil, |text| ns_string(text))
            }
        }
    }
}
extern "C" fn accessibility_value_description(this: &Object, _: Sel) -> id {
    let Some(node) = semantic_node(this) else {
        return nil;
    };
    unsafe {
        match node.descriptor.value {
            crate::AccessibilityValue::Number { text, .. } => ns_string(&text),
            crate::AccessibilityValue::Choice { selected, options } => {
                options.get(selected).map_or(nil, |text| ns_string(text))
            }
            crate::AccessibilityValue::Button | crate::AccessibilityValue::Toggle(_) => nil,
        }
    }
}
extern "C" fn accessibility_min(this: &Object, _: Sel) -> id {
    match semantic_node(this).map(|node| node.descriptor.value) {
        Some(crate::AccessibilityValue::Number { min, .. }) => unsafe {
            msg_send![class!(NSNumber), numberWithDouble: min]
        },
        _ => nil,
    }
}
extern "C" fn accessibility_max(this: &Object, _: Sel) -> id {
    match semantic_node(this).map(|node| node.descriptor.value) {
        Some(crate::AccessibilityValue::Number { max, .. }) => unsafe {
            msg_send![class!(NSNumber), numberWithDouble: max]
        },
        _ => nil,
    }
}
extern "C" fn accessibility_allowed_values(this: &Object, _: Sel) -> id {
    let Some(crate::AccessibilityValue::Choice { options, .. }) =
        semantic_node(this).map(|node| node.descriptor.value)
    else {
        return nil;
    };
    unsafe {
        let array: id = msg_send![class!(NSMutableArray), array];
        for option in options {
            let _: () = msg_send![array, addObject: ns_string(&option)];
        }
        array
    }
}

extern "C" fn accessibility_press(this: &Object, _: Sel) -> BOOL {
    semantic_action(this, crate::AccessibilityAction::Press)
}
extern "C" fn accessibility_increment(this: &Object, _: Sel) -> BOOL {
    semantic_action(this, crate::AccessibilityAction::Increment)
}
extern "C" fn accessibility_decrement(this: &Object, _: Sel) -> BOOL {
    semantic_action(this, crate::AccessibilityAction::Decrement)
}
extern "C" fn accessibility_set_focus(this: &Object, _: Sel, focused: BOOL) {
    if focused == YES {
        semantic_action(this, crate::AccessibilityAction::Focus);
    }
}
extern "C" fn accessibility_set_value(this: &Object, _: Sel, value: id) {
    if value.is_null() {
        return;
    }
    let Some(data) = semantic_state(this) else {
        return;
    };
    let Some(node) = current_node(data).map(|node| node.node) else {
        return;
    };
    let descriptor = node.descriptor.clone();
    unsafe {
        match node.descriptor.value {
            crate::AccessibilityValue::Number { .. } => {
                let numeric: BOOL = msg_send![value, isKindOfClass: class!(NSNumber)];
                if numeric == YES {
                    let value: f64 = msg_send![value, doubleValue];
                    submit(
                        data,
                        &descriptor,
                        crate::AccessibilityAction::SetNumber(value),
                    );
                }
            }
            crate::AccessibilityValue::Choice { options, .. } => {
                let string: BOOL = msg_send![value, isKindOfClass: class!(NSString)];
                if string == YES {
                    // Foundation's bounded UTF-8 copy refuses oversized external values.
                    // NSUTF8StringEncoding is4 (the existing platform binding uses the same value).
                    let byte_count: NSUInteger =
                        msg_send![value, lengthOfBytesUsingEncoding: 4 as NSUInteger];
                    if byte_count > crate::MAX_ACCESSIBILITY_TEXT_BYTES as NSUInteger {
                        return;
                    }
                    let mut bytes = [0_i8; crate::MAX_ACCESSIBILITY_TEXT_BYTES + 1];
                    let copied: BOOL = msg_send![value, getCString: bytes.as_mut_ptr() maxLength: bytes.len() encoding: 4 as NSUInteger];
                    if copied == YES
                        && CStr::from_ptr(bytes.as_ptr()).to_bytes().len() == byte_count as usize
                        && let Ok(text) = CStr::from_ptr(bytes.as_ptr()).to_str()
                    {
                        let mut indices = options
                            .iter()
                            .enumerate()
                            .filter(|(_, option)| option.as_ref() == text);
                        if let Some((index, _)) = indices.next() {
                            if indices.next().is_none() {
                                submit(
                                    data,
                                    &descriptor,
                                    crate::AccessibilityAction::Choose(index),
                                );
                            }
                        }
                    }
                }
            }
            crate::AccessibilityValue::Button | crate::AccessibilityValue::Toggle(_) => {}
        }
    }
}

extern "C" fn accessibility_help(this: &Object, _: Sel) -> id {
    semantic_node(this)
        .and_then(|node| node.descriptor.status)
        .map_or(nil, |status| unsafe { ns_string(&status) })
}

#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {
    fn NSAccessibilityPostNotification(element: id, notification: id);
    static NSAccessibilityValueChangedNotification: id;
    static NSAccessibilityFocusedUIElementChangedNotification: id;
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (crate::AccessibilityLifetime, Snapshot) {
        let owner = crate::AccessibilityLifetime::default();
        let node = crate::AccessibilityNode {
            descriptor: crate::AccessibilityDescriptor {
                id: crate::AccessibilityId(12),
                revision: 4,
                lifetime: owner.weak(),
                name: "wave".into(),
                value: crate::AccessibilityValue::Choice {
                    selected: 0,
                    options: vec!["sine".into(), "triangle".into()],
                },
                enabled: true,
                focused: false,
                status: None,
            },
            bounds: Bounds::new(point(px(1.), px(2.)), size(px(30.), px(20.))),
        };
        (
            owner,
            Snapshot {
                nodes: vec![SnapshotNode {
                    node,
                    screen: [1., 2., 30., 20.],
                }],
                refused: false,
            },
        )
    }
    #[test]
    fn appkit_stateless_button_has_button_role_no_value_and_only_action_semantics() {
        let (_owner, mut snapshot) = fixture();
        let descriptor = &mut snapshot.nodes[0].node.descriptor;
        descriptor.value = crate::AccessibilityValue::Button;
        descriptor.name = "clear search".into();
        assert!(descriptor.permits(&crate::AccessibilityAction::Press));
        assert!(descriptor.permits(&crate::AccessibilityAction::Focus));
        for action in [
            crate::AccessibilityAction::Increment,
            crate::AccessibilityAction::Decrement,
            crate::AccessibilityAction::SetNumber(1.0),
            crate::AccessibilityAction::Choose(0),
        ] {
            assert!(!descriptor.permits(&action));
        }
        descriptor.enabled = false;
        assert!(!descriptor.permits(&crate::AccessibilityAction::Press));
        descriptor.enabled = true;
        let descriptor = descriptor.clone();
        let mut registry = Registry::default();
        let bridge: Arc<NativeBridge> = registry.register(13, |_| ()).unwrap();
        bridge.publish(snapshot);
        // Same existing AppKit test boundary: release the actual semantic object
        // before assertions, including when a getter returns the wrong role.
        unsafe {
            let class = Class::get("GPUISemanticControl").unwrap();
            let object: id = msg_send![class, new];
            assert!(!object.is_null());
            (*object).set_ivar(
                "semanticState",
                Box::into_raw(Box::new(ElementData {
                    bridge: bridge.clone(),
                    id: descriptor.id,
                    owner: descriptor.lifetime,
                })) as *mut c_void,
            );
            let role: id = msg_send![object, accessibilityRole];
            let expected = ns_string("AXButton");
            let button: BOOL = msg_send![role, isEqualToString: expected];
            let value: id = msg_send![object, accessibilityValue];
            let description: id = msg_send![object, accessibilityValueDescription];
            let _: () = msg_send![object, release];
            assert_eq!(button, YES);
            assert!(value.is_null() && description.is_null());
        }
    }
    #[test]
    fn appkit_boolean_getters_follow_snapshot_and_revocation() {
        let (_owner, mut snapshot) = fixture();
        snapshot.nodes[0].node.descriptor.focused = true;
        let mut registry = Registry::default();
        let bridge: Arc<NativeBridge> = registry.register(12, |_| ()).unwrap();
        let descriptor = snapshot.nodes[0].node.descriptor.clone();
        bridge.publish(snapshot);
        unsafe {
            let class = Class::get("GPUISemanticControl").unwrap();
            let object: id = msg_send![class, new];
            assert!(!object.is_null());
            (*object).set_ivar(
                "semanticState",
                Box::into_raw(Box::new(ElementData {
                    bridge: bridge.clone(),
                    id: descriptor.id,
                    owner: descriptor.lifetime,
                })) as *mut c_void,
            );
            let enabled: BOOL = msg_send![object, isAccessibilityEnabled];
            let focused: BOOL = msg_send![object, isAccessibilityFocused];
            let hidden: BOOL = msg_send![object, isAccessibilityHidden];
            let current = bridge.snapshot().unwrap();
            let mut disabled = Snapshot {
                nodes: current.nodes.clone(),
                refused: current.refused,
            };
            disabled.nodes[0].node.descriptor.enabled = false;
            disabled.nodes[0].node.descriptor.focused = false;
            bridge.publish(disabled);
            let disabled_enabled: BOOL = msg_send![object, isAccessibilityEnabled];
            let unfocused: BOOL = msg_send![object, isAccessibilityFocused];
            bridge.revoke();
            let revoked_hidden: BOOL = msg_send![object, isAccessibilityHidden];
            let revoked_enabled: BOOL = msg_send![object, isAccessibilityEnabled];
            // Release before assertions, including on a failing negative control.
            let _: () = msg_send![object, release];
            assert_eq!((enabled, focused, hidden), (YES, YES, NO));
            assert_eq!((disabled_enabled, unfocused), (NO, NO));
            assert_eq!((revoked_hidden, revoked_enabled), (YES, NO));
        }
    }
    #[test]
    fn choice_inventory_owner_revision_and_availability_refuse_stale_actions() {
        let (owner, mut snapshot) = fixture();
        let mut action = Action {
            request: crate::AccessibilityRequest {
                id: crate::AccessibilityId(12),
                revision: 4,
                action: crate::AccessibilityAction::Choose(1),
            },
            owner: owner.weak(),
            inventory: Some(vec!["sine".into(), "triangle".into()]),
        };
        assert!(permits(&snapshot, &action));
        action.inventory = Some(vec!["triangle".into(), "sine".into()]);
        assert!(!permits(&snapshot, &action));
        action.inventory = Some(vec!["sine".into(), "triangle".into()]);
        action.request.revision = 3;
        assert!(!permits(&snapshot, &action));
        action.request.revision = 4;
        let replacement = crate::AccessibilityLifetime::default();
        snapshot.nodes[0].node.descriptor.lifetime = replacement.weak();
        assert!(!permits(&snapshot, &action));
        snapshot.nodes[0].node.descriptor.lifetime = owner.weak();
        snapshot.nodes[0].node.descriptor.enabled = false;
        assert!(!permits(&snapshot, &action));
        snapshot.nodes[0].node.descriptor.enabled = true;
        drop(owner);
        assert!(!permits(&snapshot, &action));
    }
    #[test]
    fn owning_ivars_clear_before_reentrant_drop_and_cannot_be_consumed_twice() {
        // Real Objective-C ivar storage, independent of GPUI windows/display/GPU.
        static REGISTER: std::sync::Once = std::sync::Once::new();
        REGISTER.call_once(|| {
            let mut class =
                ClassDecl::new("GPUISemanticOwnerTransferTest", class!(NSObject)).unwrap();
            class.add_ivar::<*mut c_void>(DATA_IVAR);
            class.add_ivar::<*mut c_void>("semanticState");
            class.register();
        });
        struct ReentrantDrop {
            object: id,
            name: &'static CStr,
            drops: Rc<Cell<usize>>,
        }
        impl Drop for ReentrantDrop {
            fn drop(&mut self) {
                unsafe {
                    let object = &*self.object;
                    assert!(
                        (*object.get_ivar::<*mut c_void>(self.name.to_str().unwrap())).is_null()
                    );
                    assert!(object_bridge(object).is_none());
                    assert!(semantic_state(object).is_none());
                    // Simulate nested teardown during callback/cache/data destruction.
                    assert!(take_owned_ivar::<ReentrantDrop>(object, self.name).is_none());
                }
                self.drops.set(self.drops.get() + 1);
            }
        }
        unsafe {
            let class = Class::get("GPUISemanticOwnerTransferTest").unwrap();
            let object: id = msg_send![class, new];
            assert!(!object.is_null());
            let drops = Rc::new(Cell::new(0));
            assert!(take_owned_ivar::<ReentrantDrop>(&*object, c"absentOwner").is_none());
            for name in [c"semanticBridge", c"semanticState"] {
                assert!(take_owned_ivar::<ReentrantDrop>(&*object, name).is_none());
                let raw = Box::into_raw(Box::new(ReentrantDrop {
                    object,
                    name,
                    drops: drops.clone(),
                }));
                (*object).set_ivar(name.to_str().unwrap(), raw as *mut c_void);
                // Negative control: the former Box::from_raw-first ordering still exposes ownership.
                let old_order = Box::from_raw(raw);
                assert!(!(*(*object).get_ivar::<*mut c_void>(name.to_str().unwrap())).is_null());
                assert_eq!(Box::into_raw(old_order), raw); // Return ownership without running Drop.
                let taken = take_owned_ivar::<ReentrantDrop>(&*object, name).unwrap();
                assert!((*(*object).get_ivar::<*mut c_void>(name.to_str().unwrap())).is_null());
                drop(taken); // Reentrant Drop checks cleared pointer and refuses second take.
                assert!(take_owned_ivar::<ReentrantDrop>(&*object, name).is_none());
            }
            assert_eq!(drops.get(), 2);
            let _: () = msg_send![object, release];
        }
    }
    #[test]
    fn native_transport_graph_is_send_sync_but_contains_no_ui_owner() {
        fn send_sync<T: Send + Sync>() {}
        send_sync::<Snapshot>();
        send_sync::<Action>();
        send_sync::<ElementData>();
    }
}
