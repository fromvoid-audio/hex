use hex_project::ClipId;

const MAX_QUEUED_REPEATS: u8 = 32;

#[derive(Default)]
pub(super) struct DuplicateIntent {
    request_pending: bool,
    chain_active: bool,
    queued_repeats: u8,
}

pub(super) struct Completion {
    pub select: Option<ClipId>,
    pub submit: Option<ClipId>,
}

pub(super) enum Press {
    Start(ClipId),
    Queued,
    NoSelection,
}

impl DuplicateIntent {
    /// Begin a request, or remember another press behind the outstanding one.
    pub fn press(&mut self, source: Option<ClipId>) -> Result<Press, &'static str> {
        if self.request_pending {
            if !self.chain_active {
                return Err("duplicate is still completing");
            }
            if self.queued_repeats >= MAX_QUEUED_REPEATS {
                return Err("duplicate queue full · maximum 32 queued repeats");
            }
            self.queued_repeats += 1;
            return Ok(Press::Queued);
        }
        let Some(source) = source else {
            return Ok(Press::NoSelection);
        };
        self.request_pending = true;
        self.chain_active = true;
        Ok(Press::Start(source))
    }

    /// Resolve only a matching worker completion. The next duplicate always
    /// uses the exact ID returned by the preceding successful request.
    pub fn complete(&mut self, created: Option<ClipId>) -> Completion {
        let mut completion = Completion {
            select: None,
            submit: None,
        };
        if !self.request_pending {
            return completion;
        }
        self.request_pending = false;
        let Some(created) = created.filter(|_| self.chain_active) else {
            self.cancel_all();
            return completion;
        };
        completion.select = Some(created);
        if self.queued_repeats > 0 {
            self.queued_repeats -= 1;
            self.request_pending = true;
            completion.submit = Some(created);
        } else {
            self.chain_active = false;
        }
        completion
    }

    /// Drop repeats and selection intent while allowing the in-flight edit to
    /// finish safely against its already captured canonical source ID.
    pub fn cancel_chain(&mut self) {
        self.chain_active = false;
        self.queued_repeats = 0;
        if !self.request_pending {
            self.cancel_all();
        }
    }

    /// Clear state when admission fails before the worker can report completion.
    pub fn reject(&mut self) {
        self.cancel_all();
    }

    pub fn is_pending(&self) -> bool {
        self.request_pending
    }

    fn cancel_all(&mut self) {
        self.request_pending = false;
        self.chain_active = false;
        self.queued_repeats = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_project::{Command, Created, Editor, Project};

    fn sample_clips(count: usize) -> Vec<ClipId> {
        let mut editor = Editor::new(Project::new(48_000).expect("project"), 1).expect("editor");
        let track = match editor
            .execute(Command::AddTrack {
                name: "track".into(),
            })
            .expect("track")
        {
            Created::Track(track) => track,
            other => panic!("unexpected result: {other:?}"),
        };
        let asset = match editor
            .execute(Command::AddAsset {
                path: "source.wav".into(),
                sample_rate: 48_000,
                frames: 1,
            })
            .expect("asset")
        {
            Created::Asset(asset) => asset,
            other => panic!("unexpected result: {other:?}"),
        };
        (0..count)
            .map(|_| {
                match editor
                    .execute(Command::PlaceClip {
                        track,
                        asset,
                        start: 0,
                        duration: 1,
                        source_offset: 0,
                        source_length: 1,
                        looping: false,
                    })
                    .expect("clip")
                {
                    Created::Clip(clip) => clip,
                    other => panic!("unexpected result: {other:?}"),
                }
            })
            .collect()
    }

    #[test]
    fn repeated_presses_chain_from_each_worker_returned_id() {
        let [source, first, second, third] = sample_clips(4).try_into().expect("four clips");
        let mut intent = DuplicateIntent::default();
        assert!(matches!(intent.press(Some(source)), Ok(Press::Start(id)) if id == source));
        assert!(matches!(intent.press(Some(source)), Ok(Press::Queued)));
        assert!(matches!(intent.press(Some(source)), Ok(Press::Queued)));

        let first_result = intent.complete(Some(first));
        assert_eq!(first_result.select, Some(first));
        assert_eq!(first_result.submit, Some(first));
        let second_result = intent.complete(Some(second));
        assert_eq!(second_result.select, Some(second));
        assert_eq!(second_result.submit, Some(second));
        let third_result = intent.complete(Some(third));
        assert_eq!(third_result.select, Some(third));
        assert_eq!(third_result.submit, None);
        assert!(!intent.is_pending());
    }

    #[test]
    fn repeat_count_is_bounded_and_cancel_or_rejection_clears_it() {
        let ids = sample_clips(36);
        let source = ids[0];
        let returned = ids[1];
        let mut intent = DuplicateIntent::default();
        assert!(matches!(intent.press(Some(source)), Ok(Press::Start(id)) if id == source));
        for index in 0..100 {
            if index < usize::from(MAX_QUEUED_REPEATS) {
                assert!(matches!(intent.press(Some(source)), Ok(Press::Queued)));
            } else {
                assert!(matches!(
                    intent.press(Some(source)),
                    Err("duplicate queue full · maximum 32 queued repeats")
                ));
            }
        }
        assert_eq!(intent.queued_repeats, MAX_QUEUED_REPEATS);
        let next = intent.complete(Some(returned));
        assert_eq!(next.submit, Some(returned));
        for created in ids.iter().copied().skip(2).take(31) {
            let result = intent.complete(Some(created));
            assert_eq!(result.submit, Some(created));
        }
        let last = ids[35];
        assert_eq!(intent.complete(Some(last)).submit, None);
        assert!(!intent.is_pending());

        assert!(matches!(intent.press(Some(source)), Ok(Press::Start(id)) if id == source));
        assert!(matches!(intent.press(Some(source)), Ok(Press::Queued)));
        intent.cancel_chain();
        assert!(intent.is_pending());
        assert!(matches!(
            intent.press(Some(source)),
            Err("duplicate is still completing")
        ));
        let cancelled = intent.complete(Some(returned));
        assert_eq!(cancelled.select, None);
        assert_eq!(cancelled.submit, None);
        assert!(!intent.is_pending());

        assert!(matches!(intent.press(Some(source)), Ok(Press::Start(id)) if id == source));
        intent.reject();
        assert!(!intent.is_pending());
        assert!(matches!(intent.press(Some(source)), Ok(Press::Start(id)) if id == source));
    }
}
