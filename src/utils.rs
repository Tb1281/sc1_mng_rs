#![allow(non_snake_case, non_camel_case_types, non_upper_case_globals)]

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use windows::{
    core::{w, Owned, HSTRING, PWSTR},
    Wdk::{
        Foundation::{NtQueryObject, OBJECT_INFORMATION_CLASS, OBJECT_NAME_INFORMATION},
        System::Threading::{NtQueryInformationProcess, ProcessHandleInformation},
    },
    Win32::{
        Foundation::{
            CloseHandle, DuplicateHandle, GetLastError, DUPLICATE_CLOSE_SOURCE,
            DUPLICATE_SAME_ACCESS, ERROR_ALREADY_EXISTS, HANDLE, STATUS_INFO_LENGTH_MISMATCH,
            STATUS_SUCCESS, STILL_ACTIVE,
        },
        System::{
            Com::{CoCreateInstance, CLSCTX_INPROC_SERVER},
            ProcessStatus::{EnumProcesses, GetModuleBaseNameW},
            Threading::{
                CreateMutexW, CreateProcessW, GetCurrentProcess, GetExitCodeProcess, OpenProcess,
                CREATE_NEW_CONSOLE, CREATE_NO_WINDOW, PROCESS_ALL_ACCESS, PROCESS_INFORMATION,
                STARTUPINFOW,
            },
        },
        UI::Shell::{Common::COMDLG_FILTERSPEC, FileOpenDialog, IFileDialog, SIGDN_FILESYSPATH},
    },
};

pub struct SC1ProcessInfo {
    pub pid: u32,
    pub handle: Owned<HANDLE>,
    processed: bool,
    terminated: bool,
}
unsafe impl Send for SC1ProcessInfo {}
unsafe impl Sync for SC1ProcessInfo {}
impl PartialEq for SC1ProcessInfo {
    fn eq(&self, other: &Self) -> bool {
        self.pid == other.pid
    }
}
impl Eq for SC1ProcessInfo {}
impl SC1ProcessInfo {
    pub fn new(pid: u32, handle: HANDLE) -> Self {
        Self {
            pid,
            handle: unsafe { Owned::new(handle) },
            processed: false,
            terminated: false,
        }
    }

    pub fn set_processed(&mut self) {
        self.processed = true;
    }

    pub fn set_terminated(&mut self) {
        self.terminated = true;
    }

    pub fn get_processed(&self) -> bool {
        self.processed
    }

    pub fn get_terminated(&self) -> bool {
        self.terminated
    }
}

#[repr(C)]
struct PROCESS_HANDLE_SNAPSHOT_INFORMATION {
    pub NumberOfHandles: usize,
    pub Reserved: usize,
}

#[repr(C)]
struct PROCESS_HANDLE_TABLE_ENTRY_INFO {
    pub HandleValue: HANDLE,
    pub HandleCount: usize,
    pub PointerCount: usize,
    pub GrantedAccess: u32,
    pub ObjectTypeIndex: u32,
    pub HandleAttributes: u32,
    pub Reserved: u32,
}

const ObjectNameInformation: OBJECT_INFORMATION_CLASS = OBJECT_INFORMATION_CLASS(1i32);

pub fn get_mutex() -> bool {
    unsafe {
        CreateMutexW(None, false, w!("SC1_MNG")).is_ok() && GetLastError() != ERROR_ALREADY_EXISTS
    }
}

pub fn get_path() -> Option<String> {
    unsafe {
        let dialog =
            CoCreateInstance::<_, IFileDialog>(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;

        dialog.SetTitle(w!("스타크래프트 실행 파일 선택")).ok()?;
        dialog
            .SetFileTypes(&[COMDLG_FILTERSPEC {
                pszName: w!("StarCraft.exe"),
                pszSpec: w!("StarCraft.exe"),
            }])
            .ok()?;
        dialog.Show(None).ok()?;
        dialog
            .GetResult()
            .and_then(|item| item.GetDisplayName(SIGDN_FILESYSPATH))
            .ok()
            .and_then(|item| item.to_string().ok())
    }
}

pub fn to_log(log: &str) -> String {
    format!("[{}] {}", chrono::Local::now().format("%H:%M:%S"), log)
}

pub fn save_log(logs: &[String]) {
    let file_path = format!("{}.txt", chrono::Local::now().format("%Y-%m-%d"));
    let mut vec = Vec::from(logs);

    tokio::spawn(async move {
        if let Ok(file) = tokio::fs::File::open(&file_path).await {
            let mut lines = BufReader::new(file).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                vec.push(line);
            }
            vec.sort();
            vec.dedup();
        }
        if let Ok(mut file) = tokio::fs::File::create(&file_path).await {
            file.write_all(vec.join("\r\n").as_bytes()).await.ok();
        }
    });
}

pub fn check_valid(handle: &Owned<HANDLE>) -> bool {
    unsafe {
        let mut code = 0;
        GetExitCodeProcess(**handle, &mut code).is_ok() && code == STILL_ACTIVE.0 as _
    }
}

pub fn process_handles() -> Vec<SC1ProcessInfo> {
    let mut process_list_size = 1024;
    let mut bytes_returned = 0;
    let mut processes = vec![0u32; process_list_size];
    loop {
        if unsafe {
            EnumProcesses(
                processes.as_mut_ptr(),
                process_list_size as u32,
                &mut bytes_returned,
            )
        }
        .is_ok()
        {
            break;
        }

        process_list_size *= 2;
        processes.resize(process_list_size, 0);
    }

    processes[..bytes_returned as usize]
        .iter()
        .filter_map(|&pid| {
            if pid > 0 {
                let handle = unsafe { OpenProcess(PROCESS_ALL_ACCESS, false, pid) }.ok()?;
                if !handle.is_invalid() {
                    let buffer = &mut [0u16; 512];
                    let bl = unsafe { GetModuleBaseNameW(handle, None, buffer) };
                    if bl > 0
                        && HSTRING::from_wide(&buffer[..bl as usize])
                            .is_ok_and(|h| h.to_string().to_lowercase() == "starcraft.exe")
                    {
                        Some(SC1ProcessInfo::new(pid, handle))
                    } else {
                        unsafe { CloseHandle(handle) }.ok()?;
                        None
                    }
                } else {
                    unsafe { CloseHandle(handle) }.ok()?;
                    None
                }
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
}

pub fn query_child(child: &SC1ProcessInfo) -> Option<String> {
    let mut buffer = Vec::new();
    let mut size = 0;

    loop {
        let status = unsafe {
            NtQueryInformationProcess(
                *child.handle,
                ProcessHandleInformation,
                buffer.as_mut_ptr() as *mut _,
                buffer.len() as u32,
                &mut size,
            )
        };

        match status {
            STATUS_INFO_LENGTH_MISMATCH => buffer.resize(size as usize, 0),
            STATUS_SUCCESS => {
                let handles_slice = unsafe {
                    let base_ptr = buffer.as_ptr() as *const PROCESS_HANDLE_SNAPSHOT_INFORMATION;
                    let handles_ptr =
                        (base_ptr.add(1) as *const u8) as *const PROCESS_HANDLE_TABLE_ENTRY_INFO;
                    std::slice::from_raw_parts(handles_ptr, (*base_ptr).NumberOfHandles)
                };

                for handle_info in handles_slice {
                    let mut new_handle = HANDLE::default();
                    if unsafe {
                        DuplicateHandle(
                            *child.handle,
                            handle_info.HandleValue,
                            GetCurrentProcess(),
                            &mut new_handle,
                            0,
                            false,
                            DUPLICATE_SAME_ACCESS,
                        )
                    }
                    .is_err()
                    {
                        continue;
                    }

                    let mut object_buf: Vec<u8> = Vec::new();
                    let mut object_len: u32 = 0;

                    loop {
                        let status = unsafe {
                            NtQueryObject(
                                new_handle,
                                ObjectNameInformation,
                                Some(object_buf.as_mut_ptr() as *mut _),
                                object_buf.len() as u32,
                                Some(&mut object_len),
                            )
                        };

                        match status {
                            STATUS_INFO_LENGTH_MISMATCH => {
                                object_buf.resize(object_len as usize, 0)
                            }
                            STATUS_SUCCESS => {
                                if unsafe { CloseHandle(new_handle) }.is_err() {
                                    break;
                                };

                                let p_object_info = unsafe {
                                    &*(object_buf.as_ptr() as *const OBJECT_NAME_INFORMATION)
                                };

                                if p_object_info.Name.Length > 0 {
                                    let name = unsafe { p_object_info.Name.Buffer.to_string() };
                                    if name.is_ok_and(|name| {
                                        name.contains("Starcraft Check For Other Instances")
                                    }) && unsafe {
                                        DuplicateHandle(
                                            *child.handle,
                                            handle_info.HandleValue,
                                            GetCurrentProcess(),
                                            &mut new_handle,
                                            0,
                                            false,
                                            DUPLICATE_CLOSE_SOURCE, // Close Source HANDLE
                                        )
                                        .and_then(|_| CloseHandle(new_handle))
                                    }
                                    .is_ok()
                                    {
                                        return Some(format!(
                                            "Closed {:?} for StarCraft.exe(PID: {})",
                                            handle_info.HandleValue, child.pid
                                        ));
                                    }
                                }
                                break;
                            }
                            _ => break,
                        } // match
                    } // loop
                } // for
                break;
            }
            _ => break,
        } // match
    } // loop
    None
}

pub fn run_sc1(path: &str, args: &[&str]) -> Option<SC1ProcessInfo> {
    let mut cmd = vec![path];
    cmd.extend_from_slice(args);
    let mut process_info = PROCESS_INFORMATION::default();
    let startup_info = STARTUPINFOW::default();

    unsafe {
        CreateProcessW(
            None,
            PWSTR(HSTRING::from(cmd.join(" ")).as_ptr() as *mut _),
            None,
            None,
            false,
            CREATE_NO_WINDOW | CREATE_NEW_CONSOLE,
            None,
            None,
            &startup_info,
            &mut process_info,
        )
        .ok()?;
        CloseHandle(process_info.hThread).ok()?;
    }

    Some(SC1ProcessInfo::new(
        process_info.dwProcessId,
        process_info.hProcess,
    ))
}
