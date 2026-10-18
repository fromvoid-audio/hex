Evidence only: NEVER build or merge this ref.
{
  "source": "8c7fffb9796b9136beef66d5ed1716f30a59bf36",
  "tree": "3f150731f47c0367aff07c61da619bf85f31019b",
  "base": "bc06d7573ffa7f4d989bdff9553ed3108f4f0818",
  "archive": "/workspace/hex-track-colors/artifacts/evidence/hex-2793-8c7fffb9-evidence-v1.tar.gz",
  "filename": "hex-2793-8c7fffb9-evidence-v1.tar.gz",
  "bytes": 87304,
  "sha256": "a87178c071dfc2e9a43688966f32a901df1a76668947dba3c91935943edd7691",
  "members": 57,
  "hashed_payloads": 56,
  "summaries": 5,
  "ledger": [
    {
      "run_id": "20261018T201920Z-56f97519",
      "commit": "3bfd94fac3809ba0a5d5d2942fc5df1f17e11eff",
      "status": "failed",
      "stale": false,
      "duration_seconds": 4.886,
      "exit_code": 1
    },
    {
      "run_id": "20261018T201939Z-9bf3911b",
      "commit": "3bfd94fac3809ba0a5d5d2942fc5df1f17e11eff",
      "status": "failed",
      "stale": false,
      "duration_seconds": 65.831,
      "exit_code": 1
    },
    {
      "run_id": "20261018T202148Z-b37fb34d",
      "commit": "fbde5ebe2aa76691fd91648ada0a8b9d72b10158",
      "status": "passed",
      "stale": false,
      "duration_seconds": 55.697,
      "exit_code": 0
    },
    {
      "run_id": "20261018T202402Z-0edf06ea",
      "commit": "46f7ad465cdc7b9daff016a809ab76cd04cb0888",
      "status": "passed",
      "stale": false,
      "duration_seconds": 144.74,
      "exit_code": 0
    },
    {
      "run_id": "20261018T202637Z-378108a1",
      "commit": "8c7fffb9796b9136beef66d5ed1716f30a59bf36",
      "status": "passed",
      "stale": false,
      "duration_seconds": 107.007,
      "exit_code": 0
    }
  ],
  "final_counts": {
    "09-context-menu.log": {
      "positive": 4,
      "failed": 0,
      "ignored": 0
    },
    "10-clip-color-cache.log": {
      "positive": 10,
      "failed": 0,
      "ignored": 0
    },
    "07-project.log": {
      "positive": 269,
      "failed": 0,
      "ignored": 0
    },
    "08-storage-colors.log": {
      "positive": 3,
      "failed": 0,
      "ignored": 0
    }
  },
  "files": [
    {
      "path": "crates/hex-project/src/appearance.rs",
      "before_sha256": "1d29ff01be879ea66844b097037eb1d91552a81c7dc7eaa6dacf13619fe9ed95",
      "after_sha256": "822e27311208f2208000a9035e52fc9cb449788855b8c322bbe9567489de20c3"
    },
    {
      "path": "crates/hex-project/src/edit.rs",
      "before_sha256": "09ef593ec25fd545286b29b7ded186ff75db1fe22abdb26f48826ca88b7aa768",
      "after_sha256": "1be84023165d0f4ce730057d8dfcb81b08785554c4fef1506ab0158ffe8d45d7"
    },
    {
      "path": "crates/hex-project/tests/automatic_track_colors.rs",
      "before_sha256": null,
      "after_sha256": "ead2dfbb3eb473f7db10986414e404979a930916c85721874e4bc244e2c9db77"
    },
    {
      "path": "crates/hex-storage/tests/appearance.rs",
      "before_sha256": "d74e70fbd31f39c25077d294a650dfa614040815eb1fb8a1722ad0ec058cfac5",
      "after_sha256": "d2d4eac44938e260b5426bc4e0265109b9931ff69cd3d241cf600ec13ea1b41f"
    },
    {
      "path": "crates/hex-storage/tests/automatic_track_colors.rs",
      "before_sha256": null,
      "after_sha256": "edd6224497c34fb7a7fc30c80db22a21bbbd8ea47b5a704c51956471f0c156b8"
    },
    {
      "path": "crates/hex-ui/src/editor/clip_content_cache_ui_test.rs",
      "before_sha256": "d9cf6f82d291a3af58e587efc0d3cabb4b76f0bf04ee30c52e37aaed9f4a2272",
      "after_sha256": "af6b3f3715b9b196eb36bdc038bfca68afcf861834ae41f9197678d43144d6cb"
    },
    {
      "path": "crates/hex-ui/src/editor/context_menu.rs",
      "before_sha256": "42a87a5d9a73f5026006715e523130e43670a14cc3b7e863881368c9f7f752ff",
      "after_sha256": "b190d84d3b8843b7b31fe42291dd0e2e7a9a34e80097230b80628a7d11fa5dde"
    },
    {
      "path": "docs/automatic-track-colors.md",
      "before_sha256": null,
      "after_sha256": "ea6d1b2eebb858dddab88b8d8a1954d393ff4fe6cb25ba4fa21b2e5720e20754"
    }
  ],
  "final_summary_sha256": "d4898cd57ec67b37a9e26ae12566b1cdeeeddeff535ee574969336474ad7262a"
}
Five original source-bound runs/all logs. Original zero-match cache command is not coverage. Final canonical corrected selector actually executes10cache tests.
