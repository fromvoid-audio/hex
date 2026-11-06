//! Safe, owner-thread-only adapter for the installed ODDSound MTS-ESP master API.
//!
//! The raw dynamic ABI and loading operations are isolated in `abi`; this API
//! exposes no native pointers and offers no arbitrary production library path.
//! Registration and publishing belong on a serialized control worker, never an
//! audio callback.

#![deny(unsafe_code)]

use std::{
    marker::PhantomData,
    rc::Rc,
    sync::atomic::{AtomicBool, Ordering},
};

// The production loader is unavailable off macOS; ABI fixtures still exercise it.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
#[allow(unsafe_code)]
mod abi;

/// Fixed system installation path used by production registration.
pub const SYSTEM_LIBRARY_PATH: &str = "/Library/Application Support/MTS-ESP/libMTS.dylib";
/// Minimum ABI version audited against the pinned ODDSound master wrapper.
pub const MINIMUM_LIBRARY_VERSION: i32 = 0x0001_0003;
/// MTS-ESP's master-side scale-name storage is 17 bytes including the NUL.
pub const MAX_SCALE_NAME_BYTES: usize = 16;
/// Number of MIDI note frequencies passed to MTS-ESP.
pub const FREQUENCY_COUNT: usize = 128;

static LOCAL_MASTER_REGISTERED: AtomicBool = AtomicBool::new(false);

/// Typed adapter errors. Errors carry no native pointers or loader-owned data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    UnsupportedPlatform,
    LibraryUnavailable,
    MissingSymbol(&'static str),
    IncompatibleVersion { found: i32, minimum: i32 },
    ExistingMaster,
    ExistingMasterWithIpc,
    LocalMasterAlreadyRegistered,
    InvalidFrequency,
    InvalidName,
    InvalidPeriodRatio,
    InvalidMapSize,
    InvalidRootMetadata,
    InvalidClientCount(i32),
}

impl core::fmt::Display for Error {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::UnsupportedPlatform => {
                formatter.write_str("MTS-ESP master is supported on macOS only")
            }
            Self::LibraryUnavailable => {
                formatter.write_str("installed MTS-ESP library is unavailable")
            }
            Self::MissingSymbol(name) => write!(formatter, "MTS-ESP library is missing {name}"),
            Self::IncompatibleVersion { found, minimum } => {
                write!(
                    formatter,
                    "MTS-ESP library version {found:#x} is older than {minimum:#x}"
                )
            }
            Self::ExistingMaster | Self::ExistingMasterWithIpc => {
                formatter.write_str("MTS-ESP reports an existing master registration")
            }
            Self::LocalMasterAlreadyRegistered => {
                formatter.write_str("this process already owns an MTS-ESP master")
            }
            Self::InvalidFrequency => {
                formatter.write_str("all MTS frequencies must be finite and positive")
            }
            Self::InvalidName => {
                formatter.write_str("scale name must be nonempty, NUL-free, and at most 16 bytes")
            }
            Self::InvalidPeriodRatio => {
                formatter.write_str("period ratio must be finite and between 1 and 16")
            }
            Self::InvalidMapSize => formatter.write_str("map size must be between 1 and 127"),
            Self::InvalidRootMetadata => {
                formatter.write_str("root key and degree must fit the map")
            }
            Self::InvalidClientCount(value) => {
                write!(formatter, "MTS-ESP returned invalid client count {value}")
            }
        }
    }
}

impl std::error::Error for Error {}

/// Validated scale mapping metadata. MTS uses signed-byte setters; `None`
/// deliberately maps to its documented `-1` unavailable sentinel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScaleMetadata {
    period_ratio: f64,
    map_size: u8,
    map_start_key: Option<u8>,
    reference_key: u8,
}

impl ScaleMetadata {
    /// Derive map start from the root MIDI key and its degree in the repeating
    /// pattern. A mathematical start below MIDI key 0 cannot be represented by
    /// MTS's signed key API, so it is published as the documented `-1` sentinel.
    pub fn from_root(
        period_ratio: f64,
        map_size: u8,
        root_key: u8,
        root_degree: u8,
    ) -> Result<Self, Error> {
        if !period_ratio.is_finite() || !(1.0..=16.0).contains(&period_ratio) {
            return Err(Error::InvalidPeriodRatio);
        }
        if map_size == 0 || map_size > i8::MAX as u8 {
            return Err(Error::InvalidMapSize);
        }
        if root_key as usize >= FREQUENCY_COUNT || root_degree >= map_size {
            return Err(Error::InvalidRootMetadata);
        }
        Ok(Self {
            period_ratio,
            map_size,
            map_start_key: root_key.checked_sub(root_degree),
            reference_key: root_key,
        })
    }
}

/// Complete validated payload for one synchronous MTS publication.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TuningPublication {
    frequencies: [f64; FREQUENCY_COUNT],
    name: [std::ffi::c_char; MAX_SCALE_NAME_BYTES + 1],
    metadata: ScaleMetadata,
}

impl TuningPublication {
    pub fn new(
        frequencies: [f64; FREQUENCY_COUNT],
        name: &str,
        metadata: ScaleMetadata,
    ) -> Result<Self, Error> {
        if frequencies
            .iter()
            .any(|frequency| !frequency.is_finite() || *frequency <= 0.0)
        {
            return Err(Error::InvalidFrequency);
        }
        if name.is_empty() || name.len() > MAX_SCALE_NAME_BYTES || name.as_bytes().contains(&0) {
            return Err(Error::InvalidName);
        }
        let mut name_buffer = [0 as std::ffi::c_char; MAX_SCALE_NAME_BYTES + 1];
        for (target, source) in name_buffer.iter_mut().zip(name.as_bytes()) {
            *target = *source as std::ffi::c_char;
        }
        Ok(Self {
            frequencies,
            name: name_buffer,
            metadata,
        })
    }
}

/// Validated connected-client count returned by the native library.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClientCount(u32);

impl ClientCount {
    pub fn get(self) -> u32 {
        self.0
    }
}

/// One scoped MTS master registration, tied to the thread where it was made.
///
/// It is intentionally neither `Send` nor `Sync`. Its private library handle
/// stays loaded through exactly one deregistration in `Drop`.
pub struct Master {
    api: abi::Api,
    _gate: LocalGate,
    registered: bool,
    _owner_thread: PhantomData<Rc<()>>,
}

impl Master {
    /// Load the fixed system library, refuse an existing master, and register
    /// this owner on the current control thread.
    pub fn register() -> Result<Self, Error> {
        Self::acquire(false)
    }

    /// Register after the user confirms that no other master is running.
    /// If the library still reports a master with IPC enabled, reset shared
    /// MTS state, including tuning tables, filters and the client count.
    pub fn recover() -> Result<Self, Error> {
        Self::acquire(true)
    }

    fn acquire(recover: bool) -> Result<Self, Error> {
        #[cfg(target_os = "macos")]
        {
            Self::register_path(std::path::Path::new(SYSTEM_LIBRARY_PATH), recover)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = recover;
            Err(Error::UnsupportedPlatform)
        }
    }

    #[cfg(any(target_os = "macos", test))]
    fn register_path(path: &std::path::Path, recover: bool) -> Result<Self, Error> {
        let gate = LocalGate::acquire()?;
        let api = abi::Api::load(path)?;
        api.check_registration(recover)?;
        api.register();
        Ok(Self {
            api,
            _gate: gate,
            registered: true,
            _owner_thread: PhantomData,
        })
    }

    /// Synchronously publish a completely validated 128-key table and its
    /// bounded metadata. Native calls can update external readers non-atomically.
    pub fn publish(&self, tuning: &TuningPublication) {
        self.api.publish(tuning);
    }

    /// Return a validated client count. Negative native values are rejected.
    pub fn client_count(&self) -> Result<ClientCount, Error> {
        let count = self.api.client_count();
        u32::try_from(count)
            .map(ClientCount)
            .map_err(|_| Error::InvalidClientCount(count))
    }
}

impl Drop for Master {
    fn drop(&mut self) {
        if self.registered {
            self.api.deregister();
            self.registered = false;
        }
    }
}

struct LocalGate;

#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
impl LocalGate {
    fn acquire() -> Result<Self, Error> {
        LOCAL_MASTER_REGISTERED
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| Self)
            .map_err(|_| Error::LocalMasterAlreadyRegistered)
    }
}

impl Drop for LocalGate {
    fn drop(&mut self) {
        LOCAL_MASTER_REGISTERED.store(false, Ordering::Release);
    }
}

#[cfg(test)]
#[allow(unsafe_code)]
mod tests {
    use super::*;
    use libloading::Library;
    use std::{
        fs,
        path::PathBuf,
        process::Command,
        sync::{
            Barrier, Mutex,
            atomic::{AtomicU64, Ordering},
            mpsc,
        },
        time::{SystemTime, UNIX_EPOCH},
    };

    static TEST_LOCK: Mutex<()> = Mutex::new(());
    static NEXT_DIR: AtomicU64 = AtomicU64::new(1);

    struct Fixture {
        directory: PathBuf,
        library: PathBuf,
        log: PathBuf,
    }

    impl Fixture {
        fn build(initial_master: bool, version: i32, omit_table: bool, ipc: Option<bool>) -> Self {
            let unique = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock before unix epoch")
                .as_nanos();
            let directory = std::env::temp_dir().join(format!(
                "mts-adapter-{}-{timestamp}-{unique}",
                std::process::id()
            ));
            fs::create_dir_all(&directory).expect("fixture directory");
            let source = directory.join("fixture.c");
            let library = directory.join("libmts_fixture.dylib");
            let log = directory.join("calls.log");
            let c_log = log
                .to_string_lossy()
                .replace('\\', "\\\\")
                .replace('"', "\\\"");
            let initial = i32::from(initial_master);
            let has_ipc = ipc.map_or(-1, i32::from);
            let omit_table = i32::from(omit_table);
            let code = format!(
                r#"
                #include <stdbool.h>
                #include <stdio.h>
                #include <stdint.h>
                static int master = {initial};
                static int clients = 3;
                static double frequencies[128];
                static const char *log_path = "{log}";
                static void record(const char *text) {{
                    FILE *file = fopen(log_path, "a");
                    if (file) {{ fputs(text, file); fclose(file); }}
                }}
                void MTS_RegisterMaster(void *owner) {{
                    record(owner == 0 ? "register:null\n" : "register:non-null\n");
                    master = 1;
                }}
                void MTS_DeregisterMaster(void) {{ record("deregister\n"); master = 0; }}
                bool MTS_HasMaster(void) {{ return master != 0; }}
                #if {has_ipc} >= 0
                bool MTS_HasIPC(void) {{ return {has_ipc} != 0; }}
                void MTS_Reinitialize(void) {{ record("reset\n"); master = 0; }}
                #endif
                int MTS_GetVersionNumber(void) {{ return {version}; }}
                int MTS_GetNumClients(void) {{ return clients; }}
                void MTS_TestSetClients(int value) {{ clients = value; }}
                #if !{omit_table}
                void MTS_SetNoteTunings(const double *values) {{
                    for (int i = 0; i < 128; ++i) frequencies[i] = values[i];
                    record("table\n");
                }}
                #endif
                void MTS_SetScaleName(const char *name) {{
                    FILE *file = fopen(log_path, "a");
                    if (file) {{ fprintf(file, "name:%s\n", name); fclose(file); }}
                }}
                void MTS_SetPeriodRatio(double ratio) {{
                    FILE *file = fopen(log_path, "a");
                    if (file) {{ fprintf(file, "period:%.9f\n", ratio); fclose(file); }}
                }}
                void MTS_SetMapSize(signed char size) {{
                    FILE *file = fopen(log_path, "a");
                    if (file) {{ fprintf(file, "size:%d\n", size); fclose(file); }}
                }}
                void MTS_SetMapStartKey(signed char key) {{
                    FILE *file = fopen(log_path, "a");
                    if (file) {{ fprintf(file, "start:%d\n", key); fclose(file); }}
                }}
                void MTS_SetRefKey(signed char key) {{
                    FILE *file = fopen(log_path, "a");
                    if (file) {{ fprintf(file, "ref:%d\n", key); fclose(file); }}
                }}
                double MTS_TestFrequency(int index) {{ return frequencies[index]; }}
                int MTS_TestMaster(void) {{ return master; }}
                __attribute__((destructor)) static void on_unload(void) {{ record("unload\n"); }}
                "#,
                log = c_log,
            );
            fs::write(&source, code).expect("write C fixture");
            let output = Command::new("cc")
                .args([
                    if cfg!(target_os = "macos") {
                        "-dynamiclib"
                    } else {
                        "-shared"
                    },
                    "-fPIC",
                    "-o",
                ])
                .arg(&library)
                .arg(&source)
                .output()
                .expect("run C compiler");
            assert!(
                output.status.success(),
                "C fixture compile failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            Self {
                directory,
                library,
                log,
            }
        }

        fn calls(&self) -> String {
            fs::read_to_string(&self.log).unwrap_or_default()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }

    fn test_symbol<T: Copy>(library: &Library, name: &'static [u8]) -> T {
        // SAFETY: symbols are exported by the controlled C fixture with these
        // exact test-only signatures and the Library remains alive in caller.
        unsafe { *library.get::<T>(name).expect("fixture test symbol") }
    }

    fn test_metadata() -> ScaleMetadata {
        ScaleMetadata::from_root(2.0, 7, 60, 0).unwrap()
    }

    fn test_frequencies() -> [f64; FREQUENCY_COUNT] {
        std::array::from_fn(|key| 110.0 * 2.0_f64.powf((key as f64 - 45.0) / 12.0))
    }

    #[test]
    fn registration_uses_null_raw_owner_refuses_foreign_and_deregisters_before_unload() {
        let _guard = TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let fixture = Fixture::build(false, MINIMUM_LIBRARY_VERSION, false, None);
        {
            let master = Master::register_path(&fixture.library, false).unwrap();
            assert_eq!(master.client_count().unwrap().get(), 3);
            assert_eq!(
                Master::register_path(&fixture.library, false).err(),
                Some(Error::LocalMasterAlreadyRegistered)
            );
            assert_eq!(fixture.calls(), "register:null\n");
        }
        assert_eq!(fixture.calls(), "register:null\nderegister\nunload\n");

        let foreign = Fixture::build(true, MINIMUM_LIBRARY_VERSION, false, None);
        assert_eq!(
            Master::register_path(&foreign.library, false).err(),
            Some(Error::ExistingMaster)
        );
        assert_eq!(foreign.calls(), "unload\n");
        let library = unsafe { Library::new(&foreign.library).unwrap() };
        let has_master: unsafe extern "C" fn() -> i32 = test_symbol(&library, b"MTS_TestMaster\0");
        // SAFETY: controlled fixture helper with no arguments and an int result.
        assert_eq!(unsafe { has_master() }, 1);
    }

    #[test]
    fn concurrent_registration_gate_admits_only_one_local_owner() {
        let _guard = TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let fixture = Fixture::build(false, MINIMUM_LIBRARY_VERSION, false, None);
        let barrier = std::sync::Arc::new(Barrier::new(3));
        let (sender, receiver) = mpsc::channel();
        let mut workers = Vec::new();
        for _ in 0..2 {
            let path = fixture.library.clone();
            let barrier = std::sync::Arc::clone(&barrier);
            let sender = sender.clone();
            workers.push(std::thread::spawn(move || {
                barrier.wait();
                let attempt = Master::register_path(&path, false);
                let error = attempt.as_ref().err().copied();
                sender.send(error).expect("send gate result");
                barrier.wait();
                drop(attempt);
            }));
        }
        drop(sender);
        barrier.wait();
        let first = receiver.recv().expect("first registration result");
        let second = receiver.recv().expect("second registration result");
        assert!(
            matches!(
                (first, second),
                (None, Some(Error::LocalMasterAlreadyRegistered))
                    | (Some(Error::LocalMasterAlreadyRegistered), None)
            ),
            "expected one registration and one local-gate refusal"
        );
        barrier.wait();
        for worker in workers {
            worker.join().expect("registration worker");
        }
        assert_eq!(fixture.calls(), "register:null\nderegister\nunload\n");
    }

    #[test]
    fn ipc_recovery_resets_only_a_reported_master_after_explicit_request() {
        let _guard = TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let stale = Fixture::build(true, MINIMUM_LIBRARY_VERSION, false, Some(true));
        assert_eq!(
            Master::register_path(&stale.library, false).err(),
            Some(Error::ExistingMasterWithIpc)
        );
        assert_eq!(stale.calls(), "unload\n");
        let master = Master::register_path(&stale.library, true).unwrap();
        assert_eq!(
            Master::register_path(&stale.library, true).err(),
            Some(Error::LocalMasterAlreadyRegistered)
        );
        drop(master);
        assert_eq!(
            stale.calls(),
            "unload\nreset\nregister:null\nderegister\nunload\n"
        );

        let vacant = Fixture::build(false, MINIMUM_LIBRARY_VERSION, false, Some(true));
        drop(Master::register_path(&vacant.library, true).unwrap());
        assert_eq!(vacant.calls(), "register:null\nderegister\nunload\n");

        for ipc in [None, Some(false)] {
            let occupied = Fixture::build(true, MINIMUM_LIBRARY_VERSION, false, ipc);
            assert_eq!(
                Master::register_path(&occupied.library, true).err(),
                Some(Error::ExistingMaster)
            );
            assert_eq!(occupied.calls(), "unload\n");
        }
    }

    #[test]
    fn publication_is_validated_before_calls_and_covers_table_and_signed_metadata() {
        let _guard = TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let fixture = Fixture::build(false, MINIMUM_LIBRARY_VERSION, false, None);
        let master = Master::register_path(&fixture.library, false).unwrap();
        let mut frequencies = test_frequencies();
        frequencies[127] = 12_000.0;
        let publication =
            TuningPublication::new(frequencies, "hex-scale", test_metadata()).unwrap();
        master.publish(&publication);
        assert_eq!(
            fixture.calls(),
            "register:null\ntable\nname:hex-scale\nperiod:2.000000000\nsize:7\nstart:60\nref:60\n"
        );
        let library = unsafe { Library::new(&fixture.library).unwrap() };
        let frequency: unsafe extern "C" fn(i32) -> f64 =
            test_symbol(&library, b"MTS_TestFrequency\0");
        // SAFETY: the fixture stores the complete 128-value table synchronously.
        for (index, expected) in frequencies.iter().enumerate() {
            assert_eq!(unsafe { frequency(index as i32) }, *expected);
        }
        drop(library);

        let mut invalid = frequencies;
        invalid[47] = f64::NAN;
        assert_eq!(
            TuningPublication::new(invalid, "bad", test_metadata()),
            Err(Error::InvalidFrequency)
        );
        assert_eq!(
            TuningPublication::new(frequencies, "name-that-is-too-long", test_metadata()),
            Err(Error::InvalidName)
        );
        assert_eq!(fixture.calls().matches("table").count(), 1);
        drop(master);

        let negative_start = ScaleMetadata::from_root(1.5, 7, 2, 3).unwrap();
        assert_eq!(negative_start.map_start_key, None);
        let negative_fixture = Fixture::build(false, MINIMUM_LIBRARY_VERSION, false, None);
        let master = Master::register_path(&negative_fixture.library, false).unwrap();
        let publication =
            TuningPublication::new(test_frequencies(), "negative-root", negative_start).unwrap();
        master.publish(&publication);
        assert!(negative_fixture.calls().contains("start:-1\n"));
        assert!(negative_fixture.calls().contains("ref:2\n"));
    }

    #[test]
    fn missing_symbols_and_old_versions_refuse_without_registering() {
        let _guard = TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let missing = Fixture::build(false, MINIMUM_LIBRARY_VERSION, true, None);
        assert_eq!(
            Master::register_path(&missing.library, false).err(),
            Some(Error::MissingSymbol("MTS_SetNoteTunings"))
        );
        assert_eq!(missing.calls(), "unload\n");

        let old = Fixture::build(false, MINIMUM_LIBRARY_VERSION - 1, false, None);
        assert_eq!(
            Master::register_path(&old.library, false).err(),
            Some(Error::IncompatibleVersion {
                found: MINIMUM_LIBRARY_VERSION - 1,
                minimum: MINIMUM_LIBRARY_VERSION,
            })
        );
        assert_eq!(old.calls(), "unload\n");
    }

    #[test]
    fn signed_map_metadata_and_client_count_boundaries_are_checked() {
        assert_eq!(
            ScaleMetadata::from_root(0.0, 7, 60, 0),
            Err(Error::InvalidPeriodRatio)
        );
        assert_eq!(
            ScaleMetadata::from_root(2.0, 128, 60, 0),
            Err(Error::InvalidMapSize)
        );
        assert_eq!(
            ScaleMetadata::from_root(2.0, 7, 60, 7),
            Err(Error::InvalidRootMetadata)
        );
        let mapped = ScaleMetadata::from_root(2.0, 7, 60, 4).unwrap();
        assert_eq!(mapped.map_start_key, Some(56));
        assert_eq!(mapped.reference_key, 60);
        let start_zero = ScaleMetadata::from_root(2.0, 7, 3, 3).unwrap();
        assert_eq!(start_zero.map_start_key, Some(0));
        let before_zero = ScaleMetadata::from_root(2.0, 7, 2, 3).unwrap();
        assert_eq!(before_zero.map_start_key, None);

        let _guard = TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let fixture = Fixture::build(false, MINIMUM_LIBRARY_VERSION, false, None);
        let master = Master::register_path(&fixture.library, false).unwrap();
        assert_eq!(master.client_count().unwrap().get(), 3);
        let library = unsafe { Library::new(&fixture.library).unwrap() };
        let set_clients: unsafe extern "C" fn(i32) = test_symbol(&library, b"MTS_TestSetClients\0");
        // SAFETY: controlled fixture helper accepts one test integer.
        unsafe { set_clients(-2) };
        assert_eq!(master.client_count(), Err(Error::InvalidClientCount(-2)));
        drop(library);
        drop(master);

        let mut publication_frequency = test_frequencies();
        publication_frequency[2] = f64::INFINITY;
        assert_eq!(
            TuningPublication::new(publication_frequency, "valid", test_metadata()),
            Err(Error::InvalidFrequency)
        );
    }
}
