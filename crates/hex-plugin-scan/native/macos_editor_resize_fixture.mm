// Exercise the SDK frame against a real AppKit content view without showing a window.
#include "macos_editor.hpp"
#include "pluginterfaces/base/keycodes.h"
#include <cassert>

using namespace hex_instance;

// Observe the actual panel policy without ordering a window into the user's
// desktop or activating the test process.
@interface HexFixturePanel : NSPanel {
@public
    BOOL test_visible;
    int hides,shows;
}
@end
@implementation HexFixturePanel
- (BOOL)isVisible { return test_visible; }
- (void)orderOut:(id)sender { (void)sender;test_visible=NO;++hides; }
- (void)orderFrontRegardless { test_visible=YES;++shows; }
@end

static void check_floating_panel() {
    auto* panel=[[HexFixturePanel alloc] initWithContentRect:NSMakeRect(0,0,320,240)
        styleMask:NSWindowStyleMaskTitled backing:NSBackingStoreBuffered defer:NO];
    assert(panel);
    EditorPanelVisibility::configure(panel);
    assert([panel isFloatingPanel] && [panel level]==NSFloatingWindowLevel && ![panel hidesOnDeactivate]);

    const auto editor=::getpid(),owner=::getppid();
    assert(is_editor_foreground_group(editor,editor,owner,nil));
    assert(is_editor_foreground_group(owner,editor,owner,nil));
    assert(!is_editor_foreground_group(0,editor,owner,nil));
    assert(!is_editor_foreground_group(-1,editor,owner,nil));

    EditorPanelVisibility visibility;
    panel->test_visible=YES;
    visibility.sync(panel,true);
    assert(panel->test_visible && panel->hides==0 && panel->shows==0);
    visibility.sync(panel,false);
    visibility.sync(panel,false);
    assert(!panel->test_visible && visibility.is_open(panel) && panel->hides==1);
    visibility.sync(panel,true);
    visibility.sync(panel,true);
    assert(panel->test_visible && panel->shows==1);
    panel->test_visible=NO; // Native close while visible.
    visibility.sync(panel,true);
    assert(!visibility.is_open(panel) && panel->shows==1);
    [panel release];
}

static void check_editor_placement() {
    const auto screen=[[NSScreen mainScreen] visibleFrame];
    auto* original=[[HexFixturePanel alloc] initWithContentRect:NSMakeRect(0,0,320,240)
        styleMask:NSWindowStyleMaskTitled backing:NSBackingStoreBuffered defer:NO];
    auto* replacement=[[HexFixturePanel alloc] initWithContentRect:NSMakeRect(0,0,480,320)
        styleMask:NSWindowStyleMaskTitled backing:NSBackingStoreBuffered defer:NO];
    assert(original && replacement && NSWidth(screen)>480);
    EditorPanelVisibility visibility;
    original->test_visible=YES;
    for(const auto dx:{32.0,86.0}) {
        // Deliberately move the old window again before a repeated handoff.
        [original setFrameTopLeftPoint:NSMakePoint(NSMinX(screen)+dx,NSMaxY(screen)-40)];
        const auto position=editor_window_position(original);
        assert(position);
        visibility.sync(original,false);
        visibility.sync(original,true);
        assert(NSEqualPoints(NSMakePoint(position->x,position->y),
            NSMakePoint(NSMinX([original frame]),NSMaxY([original frame]))));
        restore_editor_window_position(replacement,*position);
        assert(NSMinX([replacement frame])==position->x);
        assert(NSMaxY([replacement frame])==position->y);
        assert(![replacement isVisible]);
    }
    restore_editor_window_position(replacement,EditorPosition{900000,900000});
    assert(NSIntersectsRect([replacement frame],screen));
    [replacement release];
    [original release];
}

class ResizeView final : public IPlugView {
    NSWindow* window;
public:
    explicit ResizeView(NSWindow* window):window(window) {}
    int calls=0;
    int constraints=0;
    bool accepts=false;
    bool resizable=false;
    int focus_gained=0,focus_lost=0;
    int key_downs=0,key_ups=0,last_key=0,last_modifiers=0;
    bool handles_keys=false;

    tresult PLUGIN_API queryInterface(const TUID,void** out) override {
        if(out) *out=nullptr;
        return kNoInterface;
    }
    uint32 PLUGIN_API addRef() override { return 1; }
    uint32 PLUGIN_API release() override { return 1; }
    tresult PLUGIN_API isPlatformTypeSupported(FIDString) override { return kNotImplemented; }
    tresult PLUGIN_API attached(void*,FIDString) override { return kNotImplemented; }
    tresult PLUGIN_API removed() override { return kNotImplemented; }
    tresult PLUGIN_API onWheel(float) override { return kNotImplemented; }
    tresult PLUGIN_API onKeyDown(char16,int16 key,int16 modifiers) override {
        ++key_downs;last_key=key;last_modifiers=modifiers;
        return handles_keys?kResultOk:kResultFalse;
    }
    tresult PLUGIN_API onKeyUp(char16,int16 key,int16 modifiers) override {
        ++key_ups;last_key=key;last_modifiers=modifiers;
        return handles_keys?kResultOk:kResultFalse;
    }
    tresult PLUGIN_API getSize(ViewRect*) override { return kNotImplemented; }
    tresult PLUGIN_API onFocus(TBool focused) override {
        if(focused) ++focus_gained;else ++focus_lost;
        return kResultOk;
    }
    tresult PLUGIN_API setFrame(IPlugFrame*) override { return kNotImplemented; }
    tresult PLUGIN_API canResize() override { return resizable?kResultOk:kResultFalse; }
    tresult PLUGIN_API checkSizeConstraint(ViewRect* rect) override {
        ++constraints;
        if(rect->right>600) rect->right=600;
        return kResultOk;
    }
    tresult PLUGIN_API onSize(ViewRect* rect) override {
        ++calls;
        const auto content=[[window contentView] bounds].size;
        assert(NSEqualSizes(content,NSMakeSize(rect->right-rect->left,rect->bottom-rect->top)));
        return accepts?kResultOk:kInvalidArgument;
    }
};

static void check_focus(NSWindowStyleMask style) {
    auto* window=[[NSWindow alloc] initWithContentRect:NSMakeRect(0,0,320,240)
        styleMask:style backing:NSBackingStoreBuffered defer:NO];
    assert(window && ![window isVisible]);
    ResizeView view(window);
    EditorFrame host(window,&view);
    auto* delegate=[[HexVSTEditorResizeDelegate alloc] initWithEditorFrame:&host];
    assert([delegate respondsToSelector:@selector(windowDidBecomeKey:)]);
    assert([delegate respondsToSelector:@selector(windowDidResignKey:)]);
    auto* gained=[NSNotification notificationWithName:NSWindowDidBecomeKeyNotification object:window];
    auto* lost=[NSNotification notificationWithName:NSWindowDidResignKeyNotification object:window];
    [delegate windowDidBecomeKey:gained];
    [delegate windowDidBecomeKey:gained];
    assert(view.focus_gained==1 && view.focus_lost==0);
    [delegate windowDidResignKey:lost];
    [delegate windowDidResignKey:lost];
    assert(view.focus_gained==1 && view.focus_lost==1);
    [delegate windowDidBecomeKey:gained];
    host.focus_changed(false); // EditorWindow::close before detaching the view.
    assert(view.focus_gained==2 && view.focus_lost==2);
    host.invalidate();
    [delegate windowDidBecomeKey:gained];
    assert(view.focus_gained==2 && view.focus_lost==2);
    [delegate release];
    [window release];
}

@interface MarkedTextResponder : NSView
@end
@implementation MarkedTextResponder
- (BOOL)acceptsFirstResponder { return YES; }
- (BOOL)hasMarkedText { return YES; }
@end

static NSEvent* key_event(NSWindow* window,NSEventType type,unichar character,
                          unsigned short code,NSEventModifierFlags modifiers=0) {
    NSString* text=[NSString stringWithCharacters:&character length:1];
    return [NSEvent keyEventWithType:type location:NSZeroPoint modifierFlags:modifiers
        timestamp:0 windowNumber:[window windowNumber] context:nil
        characters:text charactersIgnoringModifiers:text isARepeat:NO keyCode:code];
}

static void check_keys() {
    auto* window=[[NSWindow alloc] initWithContentRect:NSMakeRect(0,0,320,240)
        styleMask:NSWindowStyleMaskTitled backing:NSBackingStoreBuffered defer:NO];
    auto* other=[[NSWindow alloc] initWithContentRect:NSMakeRect(0,0,320,240)
        styleMask:NSWindowStyleMaskTitled backing:NSBackingStoreBuffered defer:NO];
    ResizeView view(window);
    EditorFrame host(window,&view);
    auto* down=key_event(window,NSEventTypeKeyDown,NSLeftArrowFunctionKey,123,
                         NSEventModifierFlagShift|NSEventModifierFlagCommand);
    // A layout/input-source change can alter the key-up character while the
    // physical key remains the same; the SDK must receive the original key.
    auto* up=key_event(window,NSEventTypeKeyUp,'a',123);
    assert(!host.handle_key_event(down)); // Not focused.
    host.focus_changed(true);
    view.handles_keys=true;
    assert(host.handle_key_event(down));
    assert(view.key_downs==1 && view.last_key==KEY_LEFT &&
           view.last_modifiers==(kShiftKey|kCommandKey));
    view.handles_keys=false; // Ownership stays with the down even if SDK responses change.
    assert(host.handle_key_event(up) && view.key_ups==1 && view.last_key==KEY_LEFT);
    auto* f1=key_event(window,NSEventTypeKeyDown,NSF1FunctionKey,122);
    assert(!host.handle_key_event(f1) && view.last_key==KEY_F1);
    const auto initial_downs=view.key_downs;
    view.handles_keys=true;
    assert(!host.handle_key_event(f1) && view.key_downs==initial_downs);
    const auto initial_ups=view.key_ups;
    assert(!host.handle_key_event(key_event(window,NSEventTypeKeyUp,NSF1FunctionKey,122)) &&
           view.key_ups==initial_ups);
    const auto calls=view.key_downs;
    assert(!host.handle_key_event(key_event(other,NSEventTypeKeyDown,NSLeftArrowFunctionKey,123)));
    assert(!host.handle_key_event(key_event(window,NSEventTypeKeyDown,'a',0)));
    assert(view.key_downs==calls);
    view.handles_keys=false;
    assert(!host.handle_key_event(down));
    auto* marked=[[MarkedTextResponder alloc] initWithFrame:NSMakeRect(0,0,20,20)];
    [[window contentView] addSubview:marked];
    assert([window makeFirstResponder:marked]);
    const auto ups=view.key_ups;
    assert(!host.handle_key_event(up) && view.key_ups==ups);
    assert(!host.handle_key_event(down) && view.key_downs==calls+1);
    [window makeFirstResponder:nil];
    [marked removeFromSuperview];[marked release];
    view.handles_keys=true;
    assert(host.handle_key_event(down));
    host.focus_changed(false);
    assert(!host.handle_key_event(up));
    host.focus_changed(true);
    assert(!host.handle_key_event(up)); // An unmatched up is not delivered to the SDK.
    const auto final_calls=view.key_downs;
    host.invalidate();
    assert(!host.handle_key_event(down) && view.key_downs==final_calls);
    [other release];[window release];
}

int main() {
    @autoreleasepool {
        [NSApplication sharedApplication];
        check_floating_panel();
        check_editor_placement();
        ResizeView fixed(nullptr);
        assert(!(EditorFrame::window_style(fixed)&NSWindowStyleMaskResizable));
        fixed.resizable=true;
        const auto style=EditorFrame::window_style(fixed);
        assert(style&NSWindowStyleMaskResizable);
        check_focus(NSWindowStyleMaskTitled|NSWindowStyleMaskClosable);
        check_focus(style);
        check_keys();
        auto* window=[[NSWindow alloc] initWithContentRect:NSMakeRect(40,60,320,240)
            styleMask:style
            backing:NSBackingStoreBuffered defer:NO];
        assert(window && ![window isVisible]);
        ResizeView view(window);
        EditorFrame host(window,&view);
        auto* delegate=[[HexVSTEditorResizeDelegate alloc] initWithEditorFrame:&host];
        [window setDelegate:delegate];
        const auto original=[window frame];
        ViewRect requested(0,0,480,320);
        assert(host.resizeView(&view,&requested)==kInvalidArgument);
        assert(view.calls==1);
        assert(NSEqualRects([window frame],original));

        view.accepts=true;
        assert(host.resizeView(&view,&requested)==kResultOk);
        assert(view.calls==2);
        assert(NSEqualSizes([[window contentView] bounds].size,NSMakeSize(480,320)));
        const auto accepted=[window frame];
        assert(NSMinX(accepted)==NSMinX(original) && NSMaxY(accepted)==NSMaxY(original));
        assert(host.resizeView(&view,&requested)==kResultOk);
        ViewRect invalid(0,0,4097,320);
        assert(host.resizeView(&view,&invalid)==kInvalidArgument);
        assert(view.calls==2 && view.constraints==0 && NSEqualRects([window frame],accepted));

        const auto size_for=[window](int width,int height) {
            return [window frameRectForContentRect:NSMakeRect(0,0,width,height)].size;
        };
        // setFrame:display: only emits the did-resize callback.
        // Supply the live-drag proposal to the delegate before applying it.
        auto proposed=[window frame];
        proposed.size=[delegate windowWillResize:window toSize:size_for(700,400)];
        [window setFrame:proposed display:NO];
        assert(view.constraints==1 && view.calls==3);
        assert(NSEqualSizes([[window contentView] bounds].size,NSMakeSize(600,400)));

        view.accepts=false;
        proposed=[window frame];
        proposed.size=[delegate windowWillResize:window toSize:size_for(500,300)];
        [window setFrame:proposed display:NO];
        assert(view.constraints==2 && view.calls==4);
        assert(NSEqualSizes([[window contentView] bounds].size,NSMakeSize(600,400)));

        view.accepts=true;
        proposed=[window frame];
        proposed.size=[delegate windowWillResize:window toSize:size_for(500,300)];
        [window setFrame:proposed display:NO];
        assert(view.constraints==3 && view.calls==5);
        assert(NSEqualSizes([[window contentView] bounds].size,NSMakeSize(500,300)));

        [window setDelegate:nil];
        [delegate release];
        host.invalidate();
        [window release];
    }
}
