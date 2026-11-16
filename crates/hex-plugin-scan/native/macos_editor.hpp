// Main-lane AppKit window for the original isolated instance's SDK view.
#pragma once
#import <AppKit/AppKit.h>
#include "pluginterfaces/gui/iplugview.h"
#include "pluginterfaces/base/keycodes.h"
#include "macos_editor_position.hpp"
#include <array>
#include <atomic>
#include <chrono>
#include <cstdint>
#include <cstring>
#include <optional>
#include <new>
#include <libproc.h>
#include <unistd.h>

namespace hex_instance {
using namespace Steinberg;
enum class EditorState : uint8_t { Closed, Open, Unavailable, GestureActive=4 };

class EditorFrame final : public IPlugFrame {
    std::atomic<uint32> references{1};
    NSWindow* window;
    IPlugView* view;
    NSSize accepted_size;
    bool resizing=false,focused=false;
    // 0 = no down, 1 = AppKit, 2 = SDK, 3 = reentrant SDK plus AppKit.
    std::array<uint8_t,128> key_owners{};
    std::array<int16,128> key_values{};
    std::array<char16,128> key_characters{};
    std::array<uint64_t,128> key_serials{};
    uint64_t focus_epoch=0;
public:
    EditorFrame(NSWindow* parent,IPlugView* editor):window(parent),view(editor),
        accepted_size([[parent contentView] bounds].size) {}
    void invalidate() { ++focus_epoch;key_owners.fill(0);window=nil;view=nullptr; }
    void focus_changed(bool active) {
        if(![NSThread isMainThread] || !window || !view || focused==active) return;
        focused=active;
        ++focus_epoch;
        if(!active) key_owners.fill(0);
        view->onFocus(active);
    }
    // Unambiguous ASCII may be offered to the SDK when the responder is not a
    // text input client. Composition and other text remain with AppKit.
    bool handle_key_event(NSEvent* event) {
        if(![NSThread isMainThread] || !window || !view || !focused ||
           [event window]!=window || ([event type]!=NSEventTypeKeyDown &&
                                      [event type]!=NSEventTypeKeyUp)) return false;
        const auto code=[event keyCode];
        if(code>=key_owners.size()) return false;
        auto& owner=key_owners[code];
        if([event type]==NSEventTypeKeyUp && !owner) return false;
        // An unhandled down gives the complete pair, including repeats and
        // release, to AppKit. Never offer those later events to both owners.
        if(owner==1) {
            if([event type]==NSEventTypeKeyUp) { owner=0;++key_serials[code]; }
            return false;
        }
        const auto key_epoch=focus_epoch;
        const auto key_serial=key_serials[code];
        const auto appkit_pair=[&] {
            // Even an unrepresentable initial down owns its repeats and up.
            // A responder may reenter and admit a separate SDK pair first.
            if(key_epoch==focus_epoch) {
                if(!owner) { owner=1;++key_serials[code]; }
                else if(owner==2 && key_serial!=key_serials[code]) {
                    owner=3;++key_serials[code];
                }
            }
            return false;
        };
        const auto flags=[event modifierFlags];
        const int16 modifiers=(flags&NSEventModifierFlagShift?kShiftKey:0) |
            (flags&NSEventModifierFlagOption?kAlternateKey:0) |
            (flags&NSEventModifierFlagCommand?kCommandKey:0) |
            (flags&NSEventModifierFlagControl?kControlKey:0);
        if(owner==3) {
            // One physical release closes two separately admitted logical pairs.
            // Clear custody before entering plug-in code, which can reenter.
            if([event type]==NSEventTypeKeyUp) {
                const auto key=key_values[code];
                const auto character=key_characters[code];
                owner=0;++key_serials[code];
                view->onKeyUp(character,key,modifiers);
            }
            return false;
        }
        id responder=[window firstResponder];
        const bool text_responder=[responder respondsToSelector:@selector(hasMarkedText)];
        int16 key=owner?key_values[code]:0;
        char16 character=owner?key_characters[code]:0;
        if(owner==2 && character && [event type]==NSEventTypeKeyDown) {
            NSString* typed=[event characters];
            const bool ascii=[typed length]==1 && [typed characterAtIndex:0]>=0x20 &&
                [typed characterAtIndex:0]<=0x7e;
            if(text_responder || (flags&NSEventModifierFlagOption) || !ascii) {
                // End the SDK's logical pair before handing this repeat and
                // its physical release to AppKit's text input system.
                owner=0;
                const auto serial=++key_serials[code];
                const auto epoch=focus_epoch;
                view->onKeyUp(character,key,modifiers);
                if(epoch==focus_epoch) {
                    if(serial==key_serials[code] || owner==0) {
                        owner=1;++key_serials[code];
                    } else if(owner==2) {
                        owner=3;++key_serials[code];
                    }
                }
                return false;
            }
            character=[typed characterAtIndex:0];
        }
        if(!owner) {
            NSString* characters=[event charactersIgnoringModifiers];
            if([characters length]!=1) return appkit_pair();
            const unichar unmodified=[characters characterAtIndex:0];
            key=nontext_key(unmodified);
            if(!key && !text_responder && !(flags&NSEventModifierFlagOption))
                key=text_control_key(unmodified);
            if(!key) {
                // A text client owns even the first composing keystroke, before
                // hasMarkedText becomes true. Option/dead keys and multi-unit
                // or non-ASCII text likewise stay with the native input system.
                if(text_responder || (flags&NSEventModifierFlagOption)) return appkit_pair();
                NSString* typed=[event characters];
                if([typed length]!=1) return appkit_pair();
                character=[typed characterAtIndex:0];
                if(character<0x20 || character>0x7e) return appkit_pair();
                const auto upper=unmodified>='a' && unmodified<='z'?
                    unmodified-('a'-'A'):unmodified;
                key=CharToVirtualKeyCode(upper);
            }
        }
        if(text_responder && [(id<NSTextInputClient>)responder hasMarkedText] && !owner) return appkit_pair();
        if([event type]==NSEventTypeKeyDown) {
            const auto previous_owner=owner;
            const auto epoch=focus_epoch;
            const auto serial=key_serials[code];
            const auto handled=view->onKeyDown(character,key,modifiers)==kResultTrue;
            // Foreign code can enter AppKit and change focus or dispatch the
            // same physical key before returning. Do not restore an obsolete
            // owner over that newer state.
            if(epoch!=focus_epoch || serial!=key_serials[code])
                return previous_owner==2 || handled;
            if(!owner) {
                key_values[code]=key;key_characters[code]=character;
                owner=handled?2:1;++key_serials[code];
            }
            return owner==2;
        }
        const bool handled=owner==2;
        owner=0;
        ++key_serials[code];
        view->onKeyUp(character,key,modifiers);
        return handled;
    }
    static NSWindowStyleMask window_style(IPlugView& editor) {
        auto style=NSWindowStyleMaskTitled|NSWindowStyleMaskClosable;
        if(editor.canResize()==kResultOk) style|=NSWindowStyleMaskResizable;
        return style;
    }
    static bool extent(const ViewRect& rect,NSSize& size) {
        const auto width=int64(rect.right)-rect.left;
        const auto height=int64(rect.bottom)-rect.top;
        if(width<1 || height<1 || width>4096 || height>4096) return false;
        size=NSMakeSize(width,height);
        return true;
    }
    tresult PLUGIN_API queryInterface(const TUID iid,void** out) override {
        if(!out) return kInvalidArgument;
        *out=nullptr;
        if(!iid || (std::memcmp(iid,INLINE_UID_OF(IPlugFrame),16) &&
                    std::memcmp(iid,INLINE_UID_OF(FUnknown),16))) return kNoInterface;
        *out=static_cast<IPlugFrame*>(this);addRef();return kResultOk;
    }
    uint32 PLUGIN_API addRef() override { return ++references; }
    uint32 PLUGIN_API release() override {
        const auto left=--references;
        if(!left) delete this;
        return left;
    }
    tresult PLUGIN_API resizeView(IPlugView* editor,ViewRect* rect) override {
        if(![NSThread isMainThread] || !window || editor!=view || !rect || resizing) return kResultFalse;
        NSSize size{};
        if(!extent(*rect,size)) return kInvalidArgument;
        if(NSEqualSizes(size,[[window contentView] bounds].size)) return kResultOk;
        const auto accepted_frame=[window frame];
        resizing=true;
        [window setContentSize:size];
        const auto result=notify_size(*rect,accepted_frame);
        resizing=false;
        return result;
    }
    NSSize constrain_window_resize(NSSize proposed) {
        if(resizing) return proposed;
        const auto previous=[window frame].size;
        const auto content=[window contentRectForFrameRect:NSMakeRect(0,0,proposed.width,proposed.height)];
        ViewRect rect{};
        if(!view || !content_rectangle(content.size,rect)) return previous;
        resizing=true;
        const auto result=view->checkSizeConstraint(&rect);
        resizing=false;
        NSSize size{};
        if(result!=kResultOk || !extent(rect,size)) return previous;
        return [window frameRectForContentRect:NSMakeRect(0,0,size.width,size.height)].size;
    }
    void window_did_resize() {
        if(!view || resizing) return;
        const auto size=[[window contentView] bounds].size;
        if(NSEqualSizes(size,accepted_size)) return;
        const auto current=[window frame];
        auto rollback=[window frameRectForContentRect:NSMakeRect(0,0,accepted_size.width,accepted_size.height)];
        rollback.origin=NSMakePoint(NSMinX(current),NSMaxY(current)-NSHeight(rollback));
        ViewRect rect{};
        resizing=true;
        if(content_rectangle(size,rect)) notify_size(rect,rollback);
        else [window setFrame:rollback display:NO];
        resizing=false;
    }
private:
    static int16 text_control_key(unichar character) {
        switch(character) {
        case '\t': return KEY_TAB;
        case '\r': return KEY_RETURN;
        case 0x03: return KEY_ENTER;
        case 0x1b: return KEY_ESCAPE;
        case 0x7f: return KEY_BACK;
        default: return 0;
        }
    }
    static int16 nontext_key(unichar character) {
        switch(character) {
        case NSLeftArrowFunctionKey: return KEY_LEFT;
        case NSRightArrowFunctionKey: return KEY_RIGHT;
        case NSUpArrowFunctionKey: return KEY_UP;
        case NSDownArrowFunctionKey: return KEY_DOWN;
        case NSHomeFunctionKey: return KEY_HOME;
        case NSEndFunctionKey: return KEY_END;
        case NSPageUpFunctionKey: return KEY_PAGEUP;
        case NSPageDownFunctionKey: return KEY_PAGEDOWN;
        case NSInsertFunctionKey: return KEY_INSERT;
        case NSDeleteFunctionKey: return KEY_DELETE;
        default:
            if(character>=NSF1FunctionKey && character<=NSF12FunctionKey)
                return KEY_F1+(character-NSF1FunctionKey);
            if(character>=NSF13FunctionKey && character<=NSF24FunctionKey)
                return KEY_F13+(character-NSF13FunctionKey);
            return 0;
        }
    }
    static bool content_rectangle(NSSize size,ViewRect& rect) {
        if(!(size.width>=1 && size.width<=4096 && size.height>=1 && size.height<=4096))
            return false;
        rect=ViewRect(0,0,static_cast<int32>(size.width+0.5),static_cast<int32>(size.height+0.5));
        return true;
    }
    tresult notify_size(ViewRect& rect,NSRect rollback) {
        const auto result=view->onSize(&rect);
        if(result==kResultOk) accepted_size=[[window contentView] bounds].size;
        else [window setFrame:rollback display:NO];
        return result;
    }
};
} // namespace hex_instance

// AppKit's delegate is non-owning. EditorWindow detaches it before releasing
// the frame or view. The frame guards programmatic and nested resize callbacks.
@interface HexVSTEditorResizeDelegate : NSObject <NSWindowDelegate> {
    hex_instance::EditorFrame* editor_frame;
}
- (instancetype)initWithEditorFrame:(hex_instance::EditorFrame*)frame;
@end

@implementation HexVSTEditorResizeDelegate
- (instancetype)initWithEditorFrame:(hex_instance::EditorFrame*)frame {
    self=[super init];
    if(self) editor_frame=frame;
    return self;
}
- (NSSize)windowWillResize:(NSWindow*)sender toSize:(NSSize)size {
    return editor_frame->constrain_window_resize(size);
}
- (void)windowDidResize:(NSNotification*)notification {
    editor_frame->window_did_resize();
}
- (void)windowDidBecomeKey:(NSNotification*)notification {
    editor_frame->focus_changed(true);
}
- (void)windowDidResignKey:(NSNotification*)notification {
    editor_frame->focus_changed(false);
}
@end

namespace hex_instance {
// AppKit's default deactivate rule follows only the helper. Treat Hex and
// its editor helpers as one foreground group without transferring the view.
class EditorPanelVisibility {
    bool hidden_for_application=false;
public:
    static void configure(NSPanel* panel) {
        [panel setFloatingPanel:YES];
        [panel setLevel:NSFloatingWindowLevel];
        [panel setHidesOnDeactivate:NO];
    }
    bool is_open(NSPanel* panel) const {
        return panel && (hidden_for_application || [panel isVisible]);
    }
    void sync(NSPanel* panel,bool hex_is_foreground) {
        if(!is_open(panel)) return; // A user close stays closed.
        if(hex_is_foreground && hidden_for_application) {
            hidden_for_application=false;
            [panel orderFrontRegardless]; // Keep focus in the Hex window.
        } else if(!hex_is_foreground && !hidden_for_application) {
            hidden_for_application=true;
            [panel orderOut:nil];
        }
    }
    void reset() { hidden_for_application=false; }
};

inline bool is_editor_foreground_group(pid_t foreground,pid_t editor,pid_t owner,NSString* bundle_identifier) {
    if(foreground<=0) return false;
    if(foreground==editor || foreground==owner) return true;
    // Each plug-in has its own helper. Keep sibling panels visible while
    // moving between them, scoped to the same directly supervising Hex PID.
    if(owner<=0 || ![bundle_identifier length] ||
        ![bundle_identifier isEqualToString:[[NSRunningApplication currentApplication] bundleIdentifier]]) return false;
    proc_bsdinfo info{};
    return proc_pidinfo(foreground,PROC_PIDTBSDINFO,0,&info,sizeof(info))==static_cast<int>(sizeof(info)) &&
        info.pbi_ppid==static_cast<uint32_t>(owner);
}

inline std::optional<EditorPosition> editor_window_position(NSWindow* window) {
    if(!window) return std::nullopt;
    const auto frame=[window frame];
    const EditorPosition position{NSMinX(frame),NSMaxY(frame)};
    return position.valid()?std::optional<EditorPosition>(position):std::nullopt;
}

inline void restore_editor_window_position(NSWindow* window,EditorPosition position) {
    if(position.valid()) {
        const auto frame=[window frame];
        const auto content=[window contentRectForFrameRect:frame];
        const auto title_height=NSHeight(frame)-NSHeight(content);
        const auto title=NSMakeRect(position.x,position.y-title_height,NSWidth(frame),title_height);
        for(NSScreen* screen in [NSScreen screens]) {
            if(NSIntersectsRect(title,[screen visibleFrame])) {
                [window setFrameTopLeftPoint:NSMakePoint(position.x,position.y)];
                return;
            }
        }
    }
    [window center]; // The old display can disappear during replacement.
}

// Owns the window and one SDK reference. Explicit teardown precedes controller
// release. On a refused detach the isolated process retains foreign resources.
class EditorWindow {
    NSPanel* window=nil;
    IPlugView* view=nullptr;
    EditorFrame* frame=nullptr;
    HexVSTEditorResizeDelegate* resize_delegate=nil;
    EditorPanelVisibility visibility;
    const pid_t owner;
    std::chrono::steady_clock::time_point next_application_check{};
    bool attached=false,frame_set=false,quarantined=false,app_initialized=false;
public:
    explicit EditorWindow(pid_t owner):owner(owner) {}
    void quarantine() { quarantined=true; }
    bool close() {
        if(quarantined) return false;
        if(!view) return true;
        const bool was_visible=window && [window isVisible];
        if(frame) frame->focus_changed(false);
        if(resize_delegate) {
            [window setDelegate:nil];
            [resize_delegate release];resize_delegate=nil;
        }
        if(window) [window orderOut:nil];
        if(frame) frame->invalidate();
        if(attached && view->removed()!=kResultOk) { quarantine();return false; }
        attached=false;
        if(frame_set && view->setFrame(nullptr)!=kResultOk) { quarantine();return false; }
        frame_set=false;
        view->release();view=nullptr;
        if(frame) { frame->release();frame=nullptr; }
        if(window) {
            if(was_visible) [window close];
            [window release];window=nil;
        }
        visibility.reset();
        return true;
    }
    bool is_open() const { return visibility.is_open(window); }
    std::optional<EditorPosition> position() const {
        return is_open()?editor_window_position(window):std::nullopt;
    }
    std::optional<EditorState> open(IPlugView* editor,bool activate,
                                    std::optional<EditorPosition> placement=std::nullopt) {
        view=editor;
        if(!view) return EditorState::Unavailable;
        ViewRect rect{};
        NSSize size{};
        if(view->isPlatformTypeSupported(kPlatformTypeNSView)!=kResultOk ||
           view->getSize(&rect)!=kResultOk || !EditorFrame::extent(rect,size))
            return unavailable();

        if(!app_initialized) {
            [NSApplication sharedApplication];
            [NSApp setActivationPolicy:NSApplicationActivationPolicyAccessory];
            [NSApp finishLaunching];
            app_initialized=true;
        }
        const auto style=EditorFrame::window_style(*view);
        window=[[NSPanel alloc] initWithContentRect:NSMakeRect(0,0,size.width,size.height)
            styleMask:style
            backing:NSBackingStoreBuffered defer:NO];
        if(!window) return unavailable();
        EditorPanelVisibility::configure(window);
        [window setReleasedWhenClosed:NO];
        [window setTitle:@"vst3 editor"];
        if(!placement) [window center];
        frame=new(std::nothrow) EditorFrame(window,view);
        if(!frame) return unavailable();
        const auto frame_result=view->setFrame(frame);
        frame_set=frame_result==kResultOk;
        if(!frame_set && frame_result!=kNotImplemented) return unavailable();
        if(view->attached([window contentView],kPlatformTypeNSView)!=kResultOk) return unavailable();
        attached=true;
        resize_delegate=[[HexVSTEditorResizeDelegate alloc] initWithEditorFrame:frame];
        if(!resize_delegate) return unavailable();
        [window setDelegate:resize_delegate];
        // Apply the original top-left after attachment, when the SDK view
        // has supplied its final size. Ordering the panel keeps Hex focused.
        if(placement) restore_editor_window_position(window,*placement);
        if(activate) {
            [window makeKeyAndOrderFront:nil];
            [NSApp activateIgnoringOtherApps:YES];
        } else {
            [window orderFrontRegardless];
        }
        next_application_check=std::chrono::steady_clock::now()+std::chrono::milliseconds(100);
        return is_open()?std::optional<EditorState>(EditorState::Open):unavailable();
    }
    bool pump() {
        if(!window) return true;
        @autoreleasepool {
            // Bound AppKit event work before the next control/cancellation check.
            for(int n=0;n<8 && is_open();++n) {
                NSEvent* event=[NSApp nextEventMatchingMask:NSEventMaskAny untilDate:[NSDate distantPast]
                    inMode:NSDefaultRunLoopMode dequeue:YES];
                if(!event) break;
                if(!frame || !frame->handle_key_event(event)) [NSApp sendEvent:event];
            }
            if(is_open() && owner>0) {
                const auto now=std::chrono::steady_clock::now();
                if(now>=next_application_check) {
                    next_application_check=now+std::chrono::milliseconds(100);
                    auto* active=[[NSWorkspace sharedWorkspace] frontmostApplication];
                    if(active && [active processIdentifier]>0)
                        visibility.sync(window,is_editor_foreground_group(
                            [active processIdentifier],::getpid(),owner,[active bundleIdentifier]));
                }
            }
        }
        return is_open() || close();
    }
private:
    std::optional<EditorState> unavailable() {
        if(!close()) return std::nullopt;
        return EditorState::Unavailable;
    }
};
} // namespace hex_instance
