# hex-preferences

## Purpose

Persist small, versioned application preferences that are independent of any
one project, beginning with user-added browser folders.

## Dependencies

serde and serde_json encode the bounded versioned document; std handles native
path representations, bounded reads, and atomic same-directory publication.

## Does not own

Project documents or history, browser listings, UI layout/navigation, audio
state, plugin state, or synchronous filesystem work on the UI thread.

User paths are retained in order, including folders that are currently missing.
UTF-8 paths use portable strings; non-UTF-8 paths use native Unix bytes or
Windows UTF-16 code units and produce an explicit error on another platform.
Documents are capped at 64 KiB, 14 user-added places, 4 KiB per path and 8 KiB
of combined native path data. Saves use a synced temporary file in the destination
directory and atomically rename it into place.
