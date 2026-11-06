// Exercise the SDK frame against a real AppKit content view without showing a window.
#include "macos_editor.hpp"
#include <cassert>

using namespace hex_instance;

class ResizeView final : public IPlugView {
    NSWindow* window;
public:
    explicit ResizeView(NSWindow* window):window(window) {}
    int calls=0;
    int constraints=0;
    bool accepts=false;
    bool resizable=false;

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
    tresult PLUGIN_API onKeyDown(char16,int16,int16) override { return kNotImplemented; }
    tresult PLUGIN_API onKeyUp(char16,int16,int16) override { return kNotImplemented; }
    tresult PLUGIN_API getSize(ViewRect*) override { return kNotImplemented; }
    tresult PLUGIN_API onFocus(TBool) override { return kNotImplemented; }
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

int main() {
    @autoreleasepool {
        [NSApplication sharedApplication];
        ResizeView fixed(nullptr);
        assert(!(EditorFrame::window_style(fixed)&NSWindowStyleMaskResizable));
        fixed.resizable=true;
        const auto style=EditorFrame::window_style(fixed);
        assert(style&NSWindowStyleMaskResizable);
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
