//! Audited raw dynamic-library boundary. No ABI function or pointer escapes.

use crate::{Error, MINIMUM_LIBRARY_VERSION, TuningPublication};
use libloading::Library;
use std::{
    ffi::{c_char, c_int, c_void},
    path::Path,
};

type RegisterMaster = unsafe extern "C" fn(*mut c_void);
type DeregisterMaster = unsafe extern "C" fn();
type HasMaster = unsafe extern "C" fn() -> bool;
type HasIpc = unsafe extern "C" fn() -> bool;
type Reinitialize = unsafe extern "C" fn();
type GetVersionNumber = unsafe extern "C" fn() -> c_int;
type GetNumClients = unsafe extern "C" fn() -> c_int;
type SetNoteTunings = unsafe extern "C" fn(*const f64);
type SetScaleName = unsafe extern "C" fn(*const c_char);
type SetPeriodRatio = unsafe extern "C" fn(f64);
type SetSignedByte = unsafe extern "C" fn(i8);

pub(crate) struct Api {
    _library: Library,
    register_master: RegisterMaster,
    deregister_master: DeregisterMaster,
    has_master: HasMaster,
    recovery: Option<(HasIpc, Reinitialize)>,
    get_num_clients: GetNumClients,
    set_note_tunings: SetNoteTunings,
    set_scale_name: SetScaleName,
    set_period_ratio: SetPeriodRatio,
    set_map_size: SetSignedByte,
    set_map_start_key: SetSignedByte,
    set_ref_key: SetSignedByte,
}

impl Api {
    pub(crate) fn load(path: &Path) -> Result<Self, Error> {
        // SAFETY: the handle is kept in `library` for at least as long as every
        // copied function pointer in this Api. The path is fixed in production;
        // tests use only a controlled local C fixture through a private seam.
        let library = unsafe { Library::new(path) }.map_err(|_| Error::LibraryUnavailable)?;
        // SAFETY: signatures match the pinned raw libMTS ABI checked against
        // ODDSound's Master/libMTSMaster.cpp at commit f214739b... .
        let register_master = unsafe {
            required::<RegisterMaster>(&library, b"MTS_RegisterMaster\0", "MTS_RegisterMaster")?
        };
        // SAFETY: see the pinned function signature above.
        let deregister_master = unsafe {
            required::<DeregisterMaster>(
                &library,
                b"MTS_DeregisterMaster\0",
                "MTS_DeregisterMaster",
            )?
        };
        // SAFETY: see the pinned function signature above.
        let has_master =
            unsafe { required::<HasMaster>(&library, b"MTS_HasMaster\0", "MTS_HasMaster")? };
        // SAFETY: the pinned wrapper declares these optional recovery symbols
        // with the signatures above. The library outlives both pointers.
        let recovery = unsafe {
            library
                .get::<HasIpc>(b"MTS_HasIPC\0")
                .ok()
                .zip(library.get::<Reinitialize>(b"MTS_Reinitialize\0").ok())
                .map(|(has_ipc, reinitialize)| (*has_ipc, *reinitialize))
        };
        // SAFETY: see the pinned function signature above.
        let get_version_number = unsafe {
            required::<GetVersionNumber>(
                &library,
                b"MTS_GetVersionNumber\0",
                "MTS_GetVersionNumber",
            )?
        };
        // SAFETY: see the pinned function signature above.
        let get_num_clients = unsafe {
            required::<GetNumClients>(&library, b"MTS_GetNumClients\0", "MTS_GetNumClients")?
        };
        // SAFETY: see the pinned function signature above.
        let set_note_tunings = unsafe {
            required::<SetNoteTunings>(&library, b"MTS_SetNoteTunings\0", "MTS_SetNoteTunings")?
        };
        // SAFETY: see the pinned function signature above.
        let set_scale_name = unsafe {
            required::<SetScaleName>(&library, b"MTS_SetScaleName\0", "MTS_SetScaleName")?
        };
        // SAFETY: see the pinned function signature above.
        let set_period_ratio = unsafe {
            required::<SetPeriodRatio>(&library, b"MTS_SetPeriodRatio\0", "MTS_SetPeriodRatio")?
        };
        // SAFETY: the public header declares each of these as signed char.
        let set_map_size =
            unsafe { required::<SetSignedByte>(&library, b"MTS_SetMapSize\0", "MTS_SetMapSize")? };
        // SAFETY: the public header declares each of these as signed char.
        let set_map_start_key = unsafe {
            required::<SetSignedByte>(&library, b"MTS_SetMapStartKey\0", "MTS_SetMapStartKey")?
        };
        // SAFETY: the public header declares each of these as signed char.
        let set_ref_key =
            unsafe { required::<SetSignedByte>(&library, b"MTS_SetRefKey\0", "MTS_SetRefKey")? };

        // SAFETY: function pointer was validated above; querying version has no
        // pointer arguments or ownership effects.
        let version = unsafe { get_version_number() };
        if version < MINIMUM_LIBRARY_VERSION {
            return Err(Error::IncompatibleVersion {
                found: version,
                minimum: MINIMUM_LIBRARY_VERSION,
            });
        }
        Ok(Self {
            _library: library,
            register_master,
            deregister_master,
            has_master,
            recovery,
            get_num_clients,
            set_note_tunings,
            set_scale_name,
            set_period_ratio,
            set_map_size,
            set_map_start_key,
            set_ref_key,
        })
    }

    pub(crate) fn has_master(&self) -> bool {
        // SAFETY: validated no-argument status function; library is held alive.
        unsafe { (self.has_master)() }
    }

    /// Return the appropriate refusal, or clear a suspected stale IPC master
    /// only when the caller has explicitly requested recovery.
    pub(crate) fn check_registration(&self, recover: bool) -> Result<(), Error> {
        if !self.has_master() {
            return Ok(());
        }
        let Some((has_ipc, reinitialize)) = self.recovery else {
            return Err(Error::ExistingMaster);
        };
        // SAFETY: validated no-argument query with the library held alive.
        if !unsafe { has_ipc() } {
            return Err(Error::ExistingMaster);
        }
        if !recover {
            return Err(Error::ExistingMasterWithIpc);
        }
        // SAFETY: the upstream contract permits reinitialization with IPC
        // enabled after explicit consent to clear a suspected stale master.
        unsafe { reinitialize() };
        if self.has_master() {
            return Err(Error::ExistingMasterWithIpc);
        }
        Ok(())
    }

    pub(crate) fn register(&self) {
        // SAFETY: raw upstream ABI accepts `void*`; Master registration wrapper
        // explicitly invokes it with null (libMTSMaster.cpp's `RegisterMaster(0)`).
        unsafe { (self.register_master)(std::ptr::null_mut()) }
    }

    pub(crate) fn deregister(&self) {
        // SAFETY: only called once by the owner after this adapter registered.
        unsafe { (self.deregister_master)() }
    }

    pub(crate) fn client_count(&self) -> c_int {
        // SAFETY: validated no-argument integer query; library is held alive.
        unsafe { (self.get_num_clients)() }
    }

    pub(crate) fn publish(&self, tuning: &TuningPublication) {
        let metadata = tuning.metadata;
        let start = metadata.map_start_key.map_or(-1, |key| key as i8);
        let reference = metadata.reference_key as i8;
        // SAFETY: these six synchronous calls use pointers into the validated
        // owned value `tuning`; no pointer is retained by this adapter. MTS does
        // not promise these updates are client-sample-atomic.
        unsafe {
            (self.set_note_tunings)(tuning.frequencies.as_ptr());
            (self.set_scale_name)(tuning.name.as_ptr());
            (self.set_period_ratio)(metadata.period_ratio);
            (self.set_map_size)(metadata.map_size as i8);
            (self.set_map_start_key)(start);
            (self.set_ref_key)(reference);
        }
    }
}

unsafe fn required<T: Copy>(
    library: &Library,
    symbol: &'static [u8],
    display_name: &'static str,
) -> Result<T, Error> {
    // SAFETY: callers provide a symbol name and exact function signature from
    // the pinned upstream header/wrapper, and retain Library in Api.
    unsafe { library.get::<T>(symbol) }
        .map(|loaded| *loaded)
        .map_err(|_| Error::MissingSymbol(display_name))
}
