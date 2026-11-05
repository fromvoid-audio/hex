// Main-lane AppKit window for the original isolated instance's SDK view.
#pragma once
#import <AppKit/AppKit.h>
#include "pluginterfaces/gui/iplugview.h"
#include <atomic>
#include <cstdint>
#include <cstring>
#include <optional>
#include <new>

namespace hex_instance {
using namespace Steinberg;
enum class EditorState : uint8_t { Closed, Open, Unavailable };

class EditorFrame final : public IPlugFrame {
    std::atomic<uint32> references{1};
    NSWindow* window;
    IPlugView* view;
    bool resizing=false;
public:
    EditorFrame(NSWindow* parent,IPlugView* editor):window(parent),view(editor) {}
    void invalidate() { window=nil;view=nullptr; }
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
        resizing=true;
        [window setContentSize:size];
        const auto result=view->onSize(rect);
        resizing=false;
        return result;
    }
};

// Owns the window and one SDK reference. Explicit teardown precedes controller
// release. On a refused detach the isolated process retains foreign resources.
class EditorWindow {
    NSWindow* window=nil;
    IPlugView* view=nullptr;
    EditorFrame* frame=nullptr;
    bool attached=false,frame_set=false,quarantined=false,app_initialized=false;
public:
    void quarantine() { quarantined=true; }
    bool close() {
        if(quarantined) return false;
        if(!view) return true;
        const bool was_visible=is_open();
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
        return true;
    }
    bool is_open() const { return window && [window isVisible]; }
    std::optional<EditorState> open(IPlugView* editor) {
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
        window=[[NSWindow alloc] initWithContentRect:NSMakeRect(0,0,size.width,size.height)
            styleMask:NSWindowStyleMaskTitled|NSWindowStyleMaskClosable
            backing:NSBackingStoreBuffered defer:NO];
        if(!window) return unavailable();
        [window setReleasedWhenClosed:NO];
        [window setTitle:@"vst3 editor"];
        [window center];
        frame=new(std::nothrow) EditorFrame(window,view);
        if(!frame) return unavailable();
        const auto frame_result=view->setFrame(frame);
        frame_set=frame_result==kResultOk;
        if(!frame_set && frame_result!=kNotImplemented) return unavailable();
        if(view->attached([window contentView],kPlatformTypeNSView)!=kResultOk) return unavailable();
        attached=true;
        [window makeKeyAndOrderFront:nil];
        [NSApp activateIgnoringOtherApps:YES];
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
                [NSApp sendEvent:event];
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
