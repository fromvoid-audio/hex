#!/usr/bin/env python3
"""Enforce the device composition boundary, not DSP/plot geometry or pixels.

This deliberately small Rust token scan ignores comments and literals, balances
function bodies, and checks named production entry points. It is an architectural
guard against accidental style forks, not a Rust compiler or a security sandbox.
"""
from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parent.parent
UI = Path("crates/hex-ui/src")
PANEL = UI / "editor/device_panel"
RAW = re.compile(r'(?:br|r)(#*)"')
QUOTED = re.compile(r'''(?:b)?"(?:\\.|[^"\\])*"|(?:b)?'(?:\\.|[^'\\\n])' ''', re.X)
WORD = re.compile(r"[A-Za-z_][A-Za-z_0-9]*|0x[0-9a-fA-F_]+|[0-9]+|::")
STYLE_FIELDS = {"background", "surface", "raised", "rail", "border", "text", "muted", "accent", "focus_marker", "dial_rail", "hover", "font_size", "header_background", "header_text"}


@dataclass(frozen=True)
class Token:
    text: str
    line: int


def tokens(source: str) -> list[Token]:
    """Rust identifiers/punctuation with nested comments and quoted data skipped."""
    result = []
    i, line = 0, 1
    while i < len(source):
        start = i
        if source.startswith("//", i):
            end = source.find("\n", i)
            i = len(source) if end < 0 else end
        elif source.startswith("/*", i):
            i += 2
            depth = 1
            while i < len(source) and depth:
                if source.startswith("/*", i):
                    depth += 1
                    i += 2
                elif source.startswith("*/", i):
                    depth -= 1
                    i += 2
                else:
                    i += 1
        else:
            raw = RAW.match(source, i)
            quoted = QUOTED.match(source, i)
            if raw:
                marker = '"' + raw.group(1)
                end = source.find(marker, raw.end())
                i = len(source) if end < 0 else end + len(marker)
            elif quoted:
                i = quoted.end()
            elif source[i].isspace():
                i += 1
            else:
                word = WORD.match(source, i)
                value = word.group() if word else source[i]
                result.append(Token(value, line))
                i += len(value)
        line += source[start:i].count("\n")
    return result


def functions(stream: list[Token]) -> dict[str, list[Token]]:
    result = {}
    for index, token in enumerate(stream[:-1]):
        if token.text != "fn":
            continue
        name = stream[index + 1].text
        opening = index + 2
        while opening < len(stream) and stream[opening].text not in {"{", ";"}:
            opening += 1
        if opening == len(stream) or stream[opening].text != "{":
            continue
        depth, end = 1, opening + 1
        while end < len(stream) and depth:
            depth += (stream[end].text == "{") - (stream[end].text == "}")
            end += 1
        if depth == 0:
            result[name] = stream[opening + 1:end - 1]
    return result


def chrome_errors(stream: list[Token], label: str, *, paint: bool = True) -> list[str]:
    errors = []
    for index, token in enumerate(stream):
        previous = stream[index - 1].text if index else ""
        following = stream[index + 1].text if index + 1 < len(stream) else ""
        if paint and previous == "." and following == "(" and (
            token.text in {"bg", "background", "border_color", "border", "border_1", "border_2", "with_header_colors", "with_device_selection"}
            or token.text.startswith(("rounded", "border_l", "border_r", "border_t", "border_b"))
        ):
            errors.append(f"{label}:{token.line}: device chrome must come from hex-widgets ({token.text})")
        type_start = index
        while type_start >= 2 and stream[type_start - 1].text == "::":
            type_start -= 2
        return_type = type_start > 0 and stream[type_start - 1].text == ">"
        if token.text in {"ControlStyle", "DeviceFrameStyle", "ActionStyle", "StyleRefinement"} and following == "{" and not return_type:
            errors.append(f"{label}:{token.line}: resolve device styles in theme/device.rs")
        if token.text.startswith("0x"):
            errors.append(f"{label}:{token.line}: device-local color literal; use a semantic theme role")
        if previous == "." and token.text in STYLE_FIELDS and following == "=":
            errors.append(f"{label}:{token.line}: style field overrides belong in theme/device.rs ({token.text})")
    return errors


def require_calls(stream: list[Token], calls: tuple[str, ...], label: str) -> list[str]:
    # A gallery child module may inherit a normal `use ... as ...` module alias.
    aliases = {
        stream[index + 2].text: "device_composition"
        for index, token in enumerate(stream[:-2])
        if token.text == "device_composition" and stream[index + 1].text == "as"
    }
    code = "".join(aliases.get(token.text, token.text) for token in stream)
    return [f"{label}: missing shared call {call}" for call in calls if call + "(" not in code]


ENTRY_POINTS = {
    UI / "editor/device_panel.rs": {
        "render_effect": ("device_frame", "theme::device_frame_style", "parameter_rows::body", "device_composition::unsupported_body"),
        "render_instrument": ("device_frame", "theme::device_frame_style", "parameter_rows::body", "device_composition::unsupported_body"),
        "action_button": ("hex_widgets::device_action", "theme::device_header_action_style"),
        "control_style": ("theme::control_style",),
        "device_layout_style": ("theme::device_style",),
    },
    PANEL / "bindings.rs": {
        "parameter_style": ("theme::device_style",),
    },
    PANEL / "parameter_rows.rs": {
        "body": ("device_composition::windowed_body", "parameter_control"),
    },
    PANEL / "return_view.rs": {
        "render_return_effect": ("device_frame", "theme::device_frame_style", "parameter_rows::body", "device_composition::unsupported_body"),
    },
}


def analyze(root: Path, *, gallery: bool = True) -> list[str]:
    errors = []
    cache = {}

    def read(path: Path) -> list[Token]:
        if path in cache:
            return cache[path]
        try:
            cache[path] = tokens((root / path).read_text())
            return cache[path]
        except OSError as error:
            errors.append(f"{path}: {error}")
            return []

    for path, entries in ENTRY_POINTS.items():
        stream = read(path)
        # Host navigation/drop-feedback owns its own paint, but every control
        # style helper still belongs to the product semantic resolver.
        errors.extend(chrome_errors(stream, str(path), paint=False))
        bodies = functions(stream)
        for name, calls in entries.items():
            label = f"{path}::{name}"
            if name not in bodies:
                errors.append(f"{label}: missing production entry point")
                continue
            errors.extend(chrome_errors(bodies[name], label))
            errors.extend(require_calls(bodies[name], calls, label))

    # Discover specialized consumers, including future dedicated panels. Generic
    # devices are covered by the mandatory wrapped/unsupported dispatch above.
    paths = sorted((root / PANEL).glob("*_panel.rs"))
    if not paths:
        errors.append(f"{PANEL}: no device panels found")
    for absolute in paths:
        path = absolute.relative_to(root)
        stream = read(path)
        bodies = functions(stream)
        if "render_body" not in bodies:
            errors.append(f"{path}: missing render_body composition entry point")
            continue
        errors.extend(chrome_errors(stream, str(path)))
        code = "".join(token.text for token in bodies["render_body"])
        if "device_composition::" not in code:
            errors.append(f"{path}::render_body: use the shared production composition seam")
        # Every effect consumer must be reachable through both owner dispatches.
        module = path.stem
        if module == "operator_panel":
            owner = UI / "editor/device_panel.rs"
            body = functions(read(owner)).get("render_instrument", [])
            errors.extend(require_calls(body, ("operator_panel::render_body",), f"{owner}::render_instrument"))
        else:
            for owner in [UI / "editor/device_panel.rs", PANEL / "return_view.rs"]:
                fn = "render_effect" if owner.name == "device_panel.rs" else "render_return_effect"
                body = functions(read(owner)).get(fn, [])
                errors.extend(require_calls(body, (f"{module}::render_body",), f"{owner}::{fn}"))

    composition = read(UI / "device_composition.rs")
    errors.extend(chrome_errors(composition, str(UI / "device_composition.rs")))
    errors.extend(require_calls(composition, ("hex_widgets::device_body", "theme::device_style"), str(UI / "device_composition.rs")))
    preview = PANEL / "operator_panel/preview.rs"
    errors.extend(chrome_errors(read(preview), str(preview)))

    if gallery:
        # Both launch targets must mount the production owner, not a gallery copy.
        # This checks the explicit module boundary, not general Rust resolution.
        for entry in (Path("crates/hex-ui/examples/layout_review.rs"), Path("crates/hex-sandbox/src/main.rs")):
            source = (root / entry).read_text() if (root / entry).is_file() else ""
            pattern = r'#\[\s*path\s*=\s*"([^"]+)"\s*\]\s*(?:#\[allow\(dead_code\)\]\s*)?mod\s+device_composition\s*;'
            reference = re.search(pattern, source)
            if not reference or (root / entry.parent / reference.group(1)).resolve() != (root / UI / "device_composition.rs").resolve():
                errors.append(f"{entry}: include the owning src/device_composition.rs module")
        path = Path("crates/hex-ui/examples/layout_review/compositions.rs")
        stream = read(path)
        errors.extend(chrome_errors(stream, str(path)))
        host = Path("crates/hex-ui/examples/layout_review/gallery.rs")
        host_stream = read(host)
        for call in ("utility", "stacked_plot", "split_body", "three_pane", "paired_rails", "centered_rails", "wrapped_body", "unsupported_body"):
            errors.extend(require_calls(host_stream + stream, (f"device_composition::{call}",), str(path)))
        # Gallery navigation may style its own host buttons, but production
        # previews must still use product-resolved styles and the shared frame.
        errors.extend(chrome_errors(host_stream, str(host), paint=False))
        errors.extend(require_calls(host_stream + stream, ("device_frame", "theme::device_frame_style", "theme::device_style"), "layout_review production preview"))
    return errors


def main() -> int:
    errors = analyze(ROOT)
    if errors:
        print("\n".join(errors), file=sys.stderr)
        return 1
    print("device composition boundary passed (production, Track/Return, fallback and gallery)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
