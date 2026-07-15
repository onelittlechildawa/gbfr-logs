use anyhow::anyhow;
use log::{info, warn};
use pelite::{
    pattern,
    pe64::{Pe, PeView},
};
use thiserror::Error;
use windows::Win32::Foundation::{CloseHandle, HANDLE, HMODULE};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Module32FirstW, Module32NextW, Process32FirstW, Process32NextW,
    MODULEENTRY32W, PROCESSENTRY32W, TH32CS_SNAPMODULE, TH32CS_SNAPMODULE32, TH32CS_SNAPPROCESS,
};

struct SnapshotHandle(HANDLE);

impl Drop for SnapshotHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

#[derive(Error, Debug)]
pub enum ProcessError {
    #[error("Process was not found with that name")]
    ProcessNotFound,
    #[error("Could not snapshot process")]
    ProcessSnapshotError(windows::core::Error),
    #[error("Could not snapshot process memory")]
    ModuleSnapshotError(windows::core::Error),
}

pub struct Process {
    pub base_address: usize,
    pub module_handle: HMODULE,
}

impl Process {
    /// Finds a process by its name.
    pub fn with_name(name: &str) -> Result<Process, ProcessError> {
        let mut candidate_pids = unsafe {
            let snapshot_handle = SnapshotHandle(
                CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)
                    .map_err(ProcessError::ProcessSnapshotError)?,
            );

            let mut process = PROCESSENTRY32W {
                dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
                ..PROCESSENTRY32W::default()
            };
            Process32FirstW(snapshot_handle.0, &mut process)
                .map_err(ProcessError::ProcessSnapshotError)?;

            let mut candidates = Vec::new();
            loop {
                let process_name = String::from_utf16_lossy(&process.szExeFile)
                    .trim_end_matches('\u{0}')
                    .to_string();

                if process_name.eq_ignore_ascii_case(name) {
                    candidates.push(process.th32ProcessID);
                }

                if Process32NextW(snapshot_handle.0, &mut process).is_err() {
                    break;
                }
            }

            candidates
        };

        if candidate_pids.is_empty() {
            return Err(ProcessError::ProcessNotFound);
        }

        // The hook is loaded inside the real game process, so prefer its PID if it
        // appears among duplicate executable names. Keep the remaining candidates
        // as fallbacks for compatibility with callers outside the injected DLL.
        let current_pid = std::process::id();
        candidate_pids.sort_by_key(|pid| *pid != current_pid);

        let mut last_snapshot_error = None;
        for pid in candidate_pids {
            info!("trying module snapshot pid={pid}");

            match Self::from_pid_and_module_name(pid, name) {
                Ok(Some(process)) => return Ok(process),
                Ok(None) => {
                    warn!("target module was not found pid={pid}; trying next candidate");
                }
                Err(error) => {
                    warn!(
                        "module snapshot failed pid={pid} error={error:?}; trying next candidate"
                    );
                    last_snapshot_error = Some(error);
                }
            }
        }

        match last_snapshot_error {
            Some(error) => Err(ProcessError::ModuleSnapshotError(error)),
            None => Err(ProcessError::ProcessNotFound),
        }
    }

    fn from_pid_and_module_name(
        pid: u32,
        name: &str,
    ) -> Result<Option<Process>, windows::core::Error> {
        unsafe {
            let module_snapshot = SnapshotHandle(CreateToolhelp32Snapshot(
                TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32,
                pid,
            )?);

            let mut module_entry = MODULEENTRY32W {
                dwSize: std::mem::size_of::<MODULEENTRY32W>() as u32,
                ..MODULEENTRY32W::default()
            };
            Module32FirstW(module_snapshot.0, &mut module_entry)?;

            loop {
                let module_name = String::from_utf16_lossy(&module_entry.szModule)
                    .trim_end_matches('\u{0}')
                    .to_string();

                if module_name.eq_ignore_ascii_case(name) {
                    return Ok(Some(Process {
                        base_address: module_entry.modBaseAddr as usize,
                        module_handle: module_entry.hModule,
                    }));
                }

                if Module32NextW(module_snapshot.0, &mut module_entry).is_err() {
                    break;
                }
            }
        }

        Ok(None)
    }

    /// Searches and returns the RVAs of the function that matches the given signature pattern.
    pub fn search_address(&self, signature_pattern: &str) -> anyhow::Result<usize> {
        let view = unsafe { PeView::module(self.module_handle.0 as *const u8) };
        let scanner = view.scanner();
        let pattern = pattern::parse(signature_pattern)?;

        let mut addrs = [0; 8];

        let mut matches = scanner.matches_code(&pattern);

        let mut first_addr = None;

        // addrs[0] = RVA of where the match was found.
        // addrs[1] = RVA of the function being called.
        while matches.next(&mut addrs) {
            first_addr = Some(self.base_address + addrs[1] as usize);
        }

        first_addr.ok_or(anyhow!(
            "Could not find match for pattern: {}",
            signature_pattern
        ))
    }

    /// Searches and returns the address where the signature itself begins.
    ///
    /// `search_address` resolves a captured call target, while this is intended for
    /// signatures that identify a function prologue directly.
    pub fn search_match_address(&self, signature_pattern: &str) -> anyhow::Result<usize> {
        let view = unsafe { PeView::module(self.module_handle.0 as *const u8) };
        let scanner = view.scanner();
        let pattern = pattern::parse(signature_pattern)?;
        let mut addrs = [0; 8];
        let mut matches = scanner.matches_code(&pattern);

        if matches.next(&mut addrs) {
            Ok(self.base_address + addrs[0] as usize)
        } else {
            Err(anyhow!(
                "Could not find match for pattern: {}",
                signature_pattern
            ))
        }
    }

    /// Searches and returns the value of the type `T` that matches the given signature pattern.
    pub fn search_slice<T>(&self, signature_pattern: &str) -> anyhow::Result<T> {
        let view = unsafe { PeView::module(self.module_handle.0 as *const u8) };
        let scanner = view.scanner();
        let pattern = pattern::parse(signature_pattern)?;
        let mut addrs = [0; 8];
        let matches = scanner.matches_code(&pattern).next(&mut addrs);

        if matches {
            let addr = self.base_address + addrs[1] as usize;
            let ptr = addr as *const T;
            Ok(unsafe { ptr.read_unaligned() })
        } else {
            Err(anyhow!(
                "Could not find match for pattern: {}",
                signature_pattern
            ))
        }
    }
}
