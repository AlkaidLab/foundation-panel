//! Windows: files a streaming client copied, put on the host clipboard as
//! virtual files (FileGroupDescriptorW + one FileContents stream per file),
//! the way Explorer pastes a zip's contents. A stream asks Sunshine for the
//! file's bytes chunk by chunk only when the paste target reads it.
//!
//! OLE clipboard objects need a single-threaded apartment with a message
//! loop for as long as they are on the clipboard, so everything here runs on
//! one thread of its own. A paste's reads are marshalled to that thread and
//! block it while a chunk is on its way, as with any slow stream.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::mem::ManuallyDrop;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use windows::core::{implement, w, Interface, Ref, BOOL, HRESULT, PWSTR};
use windows::Win32::Foundation::{
    CLIPBRD_E_CANT_OPEN, DATA_S_SAMEFORMATETC, DV_E_DVASPECT, DV_E_FORMATETC, DV_E_LINDEX,
    DV_E_TYMED, E_INVALIDARG, E_NOTIMPL, E_OUTOFMEMORY, FILETIME, HGLOBAL, LPARAM,
    OLE_E_ADVISENOTSUPPORTED, STG_E_ACCESSDENIED, STG_E_INVALIDFUNCTION, STG_E_INVALIDPOINTER,
    STG_E_READFAULT, STG_E_WRITEFAULT, S_FALSE, S_OK, WPARAM,
};
use windows::Win32::System::Com::{
    CoTaskMemAlloc, IAdviseSink, IBindCtx, IDataObject, IDataObject_Impl, IEnumFORMATETC,
    IEnumSTATDATA, ISequentialStream_Impl, IStream, IStream_Impl, DATADIR_GET, DVASPECT_CONTENT,
    FORMATETC, LOCKTYPE, STATFLAG, STATFLAG_NONAME, STATSTG, STGC, STGMEDIUM, STGMEDIUM_0,
    STGM_READ, STGTY_STREAM, STREAM_SEEK, STREAM_SEEK_CUR, STREAM_SEEK_END, STREAM_SEEK_SET,
    TYMED_HGLOBAL, TYMED_ISTREAM,
};
use windows::Win32::System::DataExchange::RegisterClipboardFormatW;
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE, GMEM_ZEROINIT,
};
use windows::Win32::System::Ole::{
    OleInitialize, OleSetClipboard, ReleaseStgMedium, DROPEFFECT_COPY,
};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Shell::{
    IDataObjectAsyncCapability, IDataObjectAsyncCapability_Impl, SHCreateStdEnumFmtEtc,
    FD_ATTRIBUTES, FD_FILESIZE, FD_PROGRESSUI, FD_UNICODE, FD_WRITESTIME, FILEDESCRIPTORW,
    FILEGROUPDESCRIPTORW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, PeekMessageW, PostThreadMessageW, TranslateMessage, MSG,
    PM_NOREMOVE, WM_APP,
};

use super::inbound::{fetch_chunk, ClientOffer, CHUNK};

#[link(name = "ole32")]
unsafe extern "system" {
    #[link_name = "OleIsCurrentClipboard"]
    fn ole_is_current_clipboard(object: *mut core::ffi::c_void) -> HRESULT;
}

const FILE_ATTRIBUTE_NORMAL: u32 = 0x80;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
/// Wakes the clipboard thread to run queued commands.
const WM_RUN_COMMANDS: u32 = WM_APP + 0x51;
/// How long a paste waits for one chunk before failing the read.
const CHUNK_WAIT: Duration = Duration::from_secs(120);

fn descriptor_format() -> u16 {
    static CF: OnceLock<u16> = OnceLock::new();
    *CF.get_or_init(|| unsafe { RegisterClipboardFormatW(w!("FileGroupDescriptorW")) as u16 })
}

fn contents_format() -> u16 {
    static CF: OnceLock<u16> = OnceLock::new();
    *CF.get_or_init(|| unsafe { RegisterClipboardFormatW(w!("FileContents")) as u16 })
}

fn drop_effect_format() -> u16 {
    static CF: OnceLock<u16> = OnceLock::new();
    *CF.get_or_init(|| unsafe { RegisterClipboardFormatW(w!("Preferred DropEffect")) as u16 })
}

fn unix_to_filetime(seconds: i64) -> FILETIME {
    // FILETIME counts 100 ns intervals since 1601-01-01.
    let ticks = ((seconds + 11_644_473_600) as u64).saturating_mul(10_000_000);
    FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    }
}

fn hglobal_medium(global: HGLOBAL) -> STGMEDIUM {
    STGMEDIUM {
        tymed: TYMED_HGLOBAL.0 as u32,
        u: STGMEDIUM_0 { hGlobal: global },
        pUnkForRelease: ManuallyDrop::new(None),
    }
}

// ---- One file's contents ----

/// Chunks fetched ahead of the reader on a thread of their own.
struct Fetcher {
    /// File offset of the next chunk the channel delivers.
    next: u64,
    rx: mpsc::Receiver<Result<Vec<u8>, String>>,
    stop: Arc<AtomicBool>,
}

impl Fetcher {
    fn start(offer: Arc<ClientOffer>, index: usize, size: u64, from: u64) -> Self {
        let (tx, rx) = mpsc::sync_channel(2);
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        std::thread::spawn(move || {
            let client = match crate::sunshine::create_transfer_https_client() {
                Ok(c) => c,
                Err(e) => {
                    let _ = tx.send(Err(e));
                    return;
                }
            };
            let mut offset = from;
            while offset < size && !stopped.load(Ordering::Acquire) {
                let length = CHUNK.min(size - offset);
                let result = tauri::async_runtime::block_on(fetch_chunk(
                    &client, &offer, index, offset, length,
                ));
                let got = match &result {
                    Ok(bytes) => bytes.len() as u64,
                    Err(_) => 0,
                };
                if tx.send(result).is_err() || got == 0 {
                    return;
                }
                offset += got;
            }
        });
        Fetcher {
            next: from,
            rx,
            stop,
        }
    }
}

impl Drop for Fetcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

struct StreamState {
    position: u64,
    /// Bytes from file offset `buffer_at`.
    buffer: Vec<u8>,
    buffer_at: u64,
    fetcher: Option<Fetcher>,
}

#[implement(IStream)]
struct FileStream {
    offer: Arc<ClientOffer>,
    index: usize,
    size: u64,
    name: Vec<u16>,
    state: Mutex<StreamState>,
}

impl FileStream {
    fn new(offer: Arc<ClientOffer>, index: usize) -> Self {
        let entry = &offer.entries[index];
        let slash = entry.path.iter().rposition(|&c| c == u16::from(b'\\'));
        let name = slash.map_or(entry.path.clone(), |i| entry.path[i + 1..].to_vec());
        let size = entry.size;
        FileStream {
            offer,
            index,
            size,
            name,
            state: Mutex::new(StreamState {
                position: 0,
                buffer: Vec::new(),
                buffer_at: 0,
                fetcher: None,
            }),
        }
    }

    /// Fills `out` unless the file ends or the transfer fails; a short read
    /// means the end of the stream to IStream callers.
    fn read_into(&self, out: &mut [u8]) -> (usize, HRESULT) {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let mut total = 0usize;
        while total < out.len() && st.position < self.size {
            let in_buffer =
                st.position >= st.buffer_at && st.position < st.buffer_at + st.buffer.len() as u64;
            if in_buffer {
                let from = (st.position - st.buffer_at) as usize;
                let n = (st.buffer.len() - from).min(out.len() - total);
                out[total..total + n].copy_from_slice(&st.buffer[from..from + n]);
                total += n;
                st.position += n as u64;
                continue;
            }
            let position = st.position;
            if st.fetcher.as_ref().map(|f| f.next) != Some(position) {
                st.fetcher = Some(Fetcher::start(
                    self.offer.clone(),
                    self.index,
                    self.size,
                    position,
                ));
            }
            let fetcher = st.fetcher.as_mut().expect("started above");
            match fetcher.rx.recv_timeout(CHUNK_WAIT) {
                Ok(Ok(bytes)) if !bytes.is_empty() => {
                    fetcher.next = position + bytes.len() as u64;
                    st.buffer_at = position;
                    st.buffer = bytes;
                }
                Ok(Ok(_)) => {
                    log::warn!("client file transfer: the file ended before its announced size");
                    st.fetcher = None;
                    return (total, STG_E_READFAULT);
                }
                Ok(Err(e)) => {
                    log::warn!("client file transfer failed: {e}");
                    st.fetcher = None;
                    return (total, STG_E_READFAULT);
                }
                Err(_) => {
                    log::warn!("client file transfer: no chunk arrived in time");
                    st.fetcher = None;
                    return (total, STG_E_READFAULT);
                }
            }
        }
        (total, if total == out.len() { S_OK } else { S_FALSE })
    }
}

impl ISequentialStream_Impl for FileStream_Impl {
    fn Read(&self, pv: *mut core::ffi::c_void, cb: u32, pcbread: *mut u32) -> HRESULT {
        if !pcbread.is_null() {
            unsafe { *pcbread = 0 };
        }
        if pv.is_null() {
            return STG_E_INVALIDPOINTER;
        }
        let out = unsafe { std::slice::from_raw_parts_mut(pv as *mut u8, cb as usize) };
        let (got, hr) = self.read_into(out);
        if !pcbread.is_null() {
            unsafe { *pcbread = got as u32 };
        }
        hr
    }

    fn Write(&self, _pv: *const core::ffi::c_void, _cb: u32, pcbwritten: *mut u32) -> HRESULT {
        if !pcbwritten.is_null() {
            unsafe { *pcbwritten = 0 };
        }
        STG_E_ACCESSDENIED
    }
}

impl IStream_Impl for FileStream_Impl {
    fn Seek(
        &self,
        dlibmove: i64,
        dworigin: STREAM_SEEK,
        plibnewposition: *mut u64,
    ) -> windows::core::Result<()> {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let target = match dworigin {
            // An absolute position is unsigned.
            STREAM_SEEK_SET => Some(dlibmove as u64),
            STREAM_SEEK_CUR => st.position.checked_add_signed(dlibmove),
            STREAM_SEEK_END => self.size.checked_add_signed(dlibmove),
            _ => return Err(STG_E_INVALIDFUNCTION.into()),
        };
        // Before the start or past 2^64: refused. Past the end is a position like
        // any other (reads there return nothing), kept as asked.
        st.position = target.ok_or_else(|| windows::core::Error::from(STG_E_INVALIDFUNCTION))?;
        if !plibnewposition.is_null() {
            unsafe { *plibnewposition = st.position };
        }
        Ok(())
    }

    fn SetSize(&self, _libnewsize: u64) -> windows::core::Result<()> {
        Err(STG_E_ACCESSDENIED.into())
    }

    fn CopyTo(
        &self,
        pstm: Ref<IStream>,
        cb: u64,
        pcbread: *mut u64,
        pcbwritten: *mut u64,
    ) -> windows::core::Result<()> {
        let target = pstm.ok()?;
        let mut buffer = vec![0u8; 256 * 1024];
        let (mut left, mut read, mut written) = (cb, 0u64, 0u64);
        let mut result = Ok(());
        while left > 0 {
            let want = (buffer.len() as u64).min(left) as usize;
            let (got, hr) = self.read_into(&mut buffer[..want]);
            read += got as u64;
            if got > 0 {
                let mut put = 0u32;
                let wr = unsafe {
                    target.Write(buffer.as_ptr() as *const _, got as u32, Some(&mut put))
                };
                written += put as u64;
                // On success the counts must match: a short write is a write fault.
                if wr.is_err() || put as usize != got {
                    result = Err(if wr.is_err() {
                        wr.into()
                    } else {
                        STG_E_WRITEFAULT.into()
                    });
                    break;
                }
            }
            if hr.is_err() {
                result = Err(hr.into());
                break;
            }
            if got < want {
                break;
            }
            left -= got as u64;
        }
        unsafe {
            if !pcbread.is_null() {
                *pcbread = read;
            }
            if !pcbwritten.is_null() {
                *pcbwritten = written;
            }
        }
        result
    }

    fn Commit(&self, _grfcommitflags: &STGC) -> windows::core::Result<()> {
        Ok(())
    }

    fn Revert(&self) -> windows::core::Result<()> {
        Ok(())
    }

    fn LockRegion(
        &self,
        _liboffset: u64,
        _cb: u64,
        _dwlocktype: &LOCKTYPE,
    ) -> windows::core::Result<()> {
        Err(STG_E_INVALIDFUNCTION.into())
    }

    fn UnlockRegion(
        &self,
        _liboffset: u64,
        _cb: u64,
        _dwlocktype: u32,
    ) -> windows::core::Result<()> {
        Err(STG_E_INVALIDFUNCTION.into())
    }

    fn Stat(&self, pstatstg: *mut STATSTG, grfstatflag: &STATFLAG) -> windows::core::Result<()> {
        if pstatstg.is_null() {
            return Err(STG_E_INVALIDPOINTER.into());
        }
        let mut stat = STATSTG {
            r#type: STGTY_STREAM.0 as u32,
            cbSize: self.size,
            grfMode: STGM_READ,
            ..Default::default()
        };
        if (grfstatflag.0 & STATFLAG_NONAME.0) == 0 {
            let bytes = (self.name.len() + 1) * 2;
            let name = unsafe { CoTaskMemAlloc(bytes) } as *mut u16;
            if name.is_null() {
                return Err(E_OUTOFMEMORY.into());
            }
            unsafe {
                std::ptr::copy_nonoverlapping(self.name.as_ptr(), name, self.name.len());
                *name.add(self.name.len()) = 0;
            }
            stat.pwcsName = PWSTR(name);
        }
        unsafe { *pstatstg = stat };
        Ok(())
    }

    fn Clone(&self) -> windows::core::Result<IStream> {
        Err(E_NOTIMPL.into())
    }
}

// ---- The clipboard object ----

#[implement(IDataObject, IDataObjectAsyncCapability)]
struct FileDataObject {
    offer: Arc<ClientOffer>,
    async_mode: AtomicBool,
    in_operation: AtomicBool,
}

impl FileDataObject {
    fn query(&self, format: &FORMATETC) -> HRESULT {
        if format.dwAspect != DVASPECT_CONTENT.0 {
            return DV_E_DVASPECT;
        }
        if format.cfFormat == descriptor_format() || format.cfFormat == drop_effect_format() {
            return if format.tymed & TYMED_HGLOBAL.0 as u32 != 0 {
                S_OK
            } else {
                DV_E_TYMED
            };
        }
        if format.cfFormat == contents_format() {
            if format.tymed & TYMED_ISTREAM.0 as u32 == 0 {
                return DV_E_TYMED;
            }
            // lindex -1 (the whole clipboard, as a clipboard viewer asks) has no contents.
            let index = format.lindex;
            if index < 0
                || index as usize >= self.offer.entries.len()
                || self.offer.entries[index as usize].dir
            {
                return DV_E_LINDEX;
            }
            return S_OK;
        }
        DV_E_FORMATETC
    }

    fn descriptor(&self) -> windows::core::Result<HGLOBAL> {
        let entries = &self.offer.entries;
        let bytes = std::mem::size_of::<FILEGROUPDESCRIPTORW>()
            + entries.len().saturating_sub(1) * std::mem::size_of::<FILEDESCRIPTORW>();
        let global = unsafe { GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, bytes)? };
        unsafe {
            let group = GlobalLock(global) as *mut FILEGROUPDESCRIPTORW;
            if group.is_null() {
                return Err(E_OUTOFMEMORY.into());
            }
            (*group).cItems = entries.len() as u32;
            let first = std::ptr::addr_of_mut!((*group).fgd) as *mut FILEDESCRIPTORW;
            for (i, entry) in entries.iter().enumerate() {
                // The shell's structures are packed: build each one here and copy it in.
                let mut flags =
                    (FD_ATTRIBUTES.0 | FD_FILESIZE.0 | FD_PROGRESSUI.0 | FD_UNICODE.0) as u32;
                let mut fd = FILEDESCRIPTORW::default();
                if entry.mtime > 0 {
                    flags |= FD_WRITESTIME.0 as u32;
                    fd.ftLastWriteTime = unix_to_filetime(entry.mtime);
                }
                fd.dwFlags = flags;
                fd.dwFileAttributes = if entry.dir {
                    FILE_ATTRIBUTE_DIRECTORY
                } else {
                    FILE_ATTRIBUTE_NORMAL
                };
                fd.nFileSizeHigh = (entry.size >> 32) as u32;
                fd.nFileSizeLow = entry.size as u32;
                let mut name = [0u16; 260];
                let units = entry.path.len().min(name.len() - 1);
                name[..units].copy_from_slice(&entry.path[..units]);
                fd.cFileName = name;
                std::ptr::write_unaligned(first.add(i), fd);
            }
            let _ = GlobalUnlock(global);
        }
        Ok(global)
    }
}

impl IDataObject_Impl for FileDataObject_Impl {
    fn GetData(&self, pformatetcin: *const FORMATETC) -> windows::core::Result<STGMEDIUM> {
        if pformatetcin.is_null() {
            return Err(E_INVALIDARG.into());
        }
        let format = unsafe { &*pformatetcin };
        let supported = self.query(format);
        if supported.is_err() {
            return Err(supported.into());
        }
        if format.cfFormat == descriptor_format() {
            return Ok(hglobal_medium(self.descriptor()?));
        }
        if format.cfFormat == drop_effect_format() {
            let global = unsafe { GlobalAlloc(GMEM_MOVEABLE, std::mem::size_of::<u32>())? };
            unsafe {
                let p = GlobalLock(global) as *mut u32;
                if p.is_null() {
                    return Err(E_OUTOFMEMORY.into());
                }
                *p = DROPEFFECT_COPY.0;
                let _ = GlobalUnlock(global);
            }
            return Ok(hglobal_medium(global));
        }
        // FileContents, checked by query().
        let stream: IStream = FileStream::new(self.offer.clone(), format.lindex as usize).into();
        Ok(STGMEDIUM {
            tymed: TYMED_ISTREAM.0 as u32,
            u: STGMEDIUM_0 {
                pstm: ManuallyDrop::new(Some(stream)),
            },
            pUnkForRelease: ManuallyDrop::new(None),
        })
    }

    fn GetDataHere(
        &self,
        _pformatetc: *const FORMATETC,
        _pmedium: *mut STGMEDIUM,
    ) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn QueryGetData(&self, pformatetc: *const FORMATETC) -> HRESULT {
        if pformatetc.is_null() {
            return E_INVALIDARG;
        }
        self.query(unsafe { &*pformatetc })
    }

    fn GetCanonicalFormatEtc(
        &self,
        _pformatectin: *const FORMATETC,
        pformatetcout: *mut FORMATETC,
    ) -> HRESULT {
        if !pformatetcout.is_null() {
            unsafe { (*pformatetcout).ptd = std::ptr::null_mut() };
        }
        DATA_S_SAMEFORMATETC
    }

    fn SetData(
        &self,
        _pformatetc: *const FORMATETC,
        pmedium: *const STGMEDIUM,
        frelease: BOOL,
    ) -> windows::core::Result<()> {
        // Explorer reports "Performed DropEffect" / "Paste Succeeded" here; nothing to keep.
        if frelease.as_bool() && !pmedium.is_null() {
            unsafe { ReleaseStgMedium(pmedium as *mut STGMEDIUM) };
        }
        Ok(())
    }

    fn EnumFormatEtc(&self, dwdirection: u32) -> windows::core::Result<IEnumFORMATETC> {
        if dwdirection != DATADIR_GET.0 as u32 {
            return Err(E_NOTIMPL.into());
        }
        let format = |cf: u16, tymed: i32| FORMATETC {
            cfFormat: cf,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: tymed as u32,
        };
        let formats = [
            format(descriptor_format(), TYMED_HGLOBAL.0),
            format(contents_format(), TYMED_ISTREAM.0),
            format(drop_effect_format(), TYMED_HGLOBAL.0),
        ];
        unsafe { SHCreateStdEnumFmtEtc(&formats) }
    }

    fn DAdvise(
        &self,
        _pformatetc: *const FORMATETC,
        _advf: u32,
        _padvsink: Ref<IAdviseSink>,
    ) -> windows::core::Result<u32> {
        Err(OLE_E_ADVISENOTSUPPORTED.into())
    }

    fn DUnadvise(&self, _dwconnection: u32) -> windows::core::Result<()> {
        Err(OLE_E_ADVISENOTSUPPORTED.into())
    }

    fn EnumDAdvise(&self) -> windows::core::Result<IEnumSTATDATA> {
        Err(OLE_E_ADVISENOTSUPPORTED.into())
    }
}

// Lets Explorer copy on a worker thread of its own instead of its UI thread.
impl IDataObjectAsyncCapability_Impl for FileDataObject_Impl {
    fn SetAsyncMode(&self, fdoopasync: BOOL) -> windows::core::Result<()> {
        self.async_mode
            .store(fdoopasync.as_bool(), Ordering::Release);
        Ok(())
    }

    fn GetAsyncMode(&self) -> windows::core::Result<BOOL> {
        Ok(self.async_mode.load(Ordering::Acquire).into())
    }

    fn StartOperation(&self, _pbcreserved: Ref<IBindCtx>) -> windows::core::Result<()> {
        self.in_operation.store(true, Ordering::Release);
        Ok(())
    }

    fn InOperation(&self) -> windows::core::Result<BOOL> {
        Ok(self.in_operation.load(Ordering::Acquire).into())
    }

    fn EndOperation(
        &self,
        _hresult: HRESULT,
        _pbcreserved: Ref<IBindCtx>,
        _dweffects: u32,
    ) -> windows::core::Result<()> {
        self.in_operation.store(false, Ordering::Release);
        Ok(())
    }
}

// ---- The clipboard thread ----

enum Command {
    Set(ClientOffer, mpsc::Sender<Result<(), String>>),
    Clear,
}

struct Worker {
    thread_id: u32,
    queue: Arc<Mutex<VecDeque<Command>>>,
}

thread_local! {
    /// The object this thread last put on the clipboard.
    static CURRENT: RefCell<Option<IDataObject>> = const { RefCell::new(None) };
}

fn run(command: Command) {
    match command {
        Command::Set(offer, reply) => {
            let object: IDataObject = FileDataObject {
                offer: Arc::new(offer),
                async_mode: AtomicBool::new(true),
                in_operation: AtomicBool::new(false),
            }
            .into();
            let mut result = Ok(());
            // Another process may hold the clipboard open for a moment.
            for _ in 0..10 {
                result = unsafe { OleSetClipboard(&object) };
                match &result {
                    Err(e) if e.code() == CLIPBRD_E_CANT_OPEN => {
                        std::thread::sleep(Duration::from_millis(20))
                    }
                    _ => break,
                }
            }
            let reply_value = match result {
                Ok(()) => {
                    CURRENT.with(|c| *c.borrow_mut() = Some(object));
                    Ok(())
                }
                Err(e) => Err(format!("OleSetClipboard failed: {e}")),
            };
            let _ = reply.send(reply_value);
        }
        Command::Clear => CURRENT.with(|c| {
            if let Some(object) = c.borrow_mut().take() {
                // S_FALSE (another program's data is on the clipboard) is a
                // success code, which the generated wrapper cannot tell apart.
                if unsafe { ole_is_current_clipboard(object.as_raw()) } == S_OK {
                    let _ = unsafe { OleSetClipboard(None) };
                }
            }
        }),
    }
}

fn worker() -> Result<&'static Worker, String> {
    static WORKER: OnceLock<Result<Worker, String>> = OnceLock::new();
    WORKER
        .get_or_init(|| {
            let queue: Arc<Mutex<VecDeque<Command>>> = Arc::new(Mutex::new(VecDeque::new()));
            let thread_queue = queue.clone();
            let (ready_tx, ready_rx) = mpsc::channel();
            std::thread::Builder::new()
                .name("client-files-clipboard".into())
                .spawn(move || {
                    if let Err(e) = unsafe { OleInitialize(None) } {
                        let _ = ready_tx.send(Err(format!("OleInitialize failed: {e}")));
                        return;
                    }
                    let mut msg = MSG::default();
                    // Create this thread's message queue before anyone posts to it.
                    let _ = unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE) };
                    let _ = ready_tx.send(Ok(unsafe { GetCurrentThreadId() }));
                    loop {
                        let got = unsafe { GetMessageW(&mut msg, None, 0, 0) };
                        if got.0 <= 0 {
                            break;
                        }
                        if msg.message == WM_RUN_COMMANDS && msg.hwnd.is_invalid() {
                            loop {
                                let next = thread_queue
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner())
                                    .pop_front();
                                match next {
                                    Some(command) => run(command),
                                    None => break,
                                }
                            }
                            continue;
                        }
                        unsafe {
                            let _ = TranslateMessage(&msg);
                            DispatchMessageW(&msg);
                        }
                    }
                })
                .map_err(|e| e.to_string())?;
            let thread_id = ready_rx
                .recv_timeout(Duration::from_secs(10))
                .map_err(|_| "the clipboard thread did not start".to_string())??;
            Ok(Worker { thread_id, queue })
        })
        .as_ref()
        .map_err(|e| e.clone())
}

fn post(command: Command) -> Result<(), String> {
    let worker = worker()?;
    worker
        .queue
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push_back(command);
    unsafe { PostThreadMessageW(worker.thread_id, WM_RUN_COMMANDS, WPARAM(0), LPARAM(0)) }
        .map_err(|e| format!("PostThreadMessageW failed: {e}"))
}

/// Put the client's files on the host clipboard.
pub fn set(offer: ClientOffer) -> Result<(), String> {
    let (tx, rx) = mpsc::channel();
    post(Command::Set(offer, tx))?;
    rx.recv_timeout(Duration::from_secs(10))
        .map_err(|_| "the clipboard thread did not answer".to_string())?
}

/// Take the client's files off the clipboard if they are still there.
#[allow(dead_code)]
pub fn clear_if_owned() {
    let _ = post(Command::Clear);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_transfer::inbound::Entry;
    use windows::Win32::System::Ole::OleGetClipboard;

    fn entry(path: &str, size: u64, dir: bool) -> Entry {
        Entry {
            path: path.encode_utf16().collect(),
            size,
            dir,
            mtime: 1_700_000_000,
        }
    }

    #[test]
    fn the_files_are_on_the_clipboard_as_explorer_reads_them() {
        let offer = ClientOffer {
            sid: 7,
            id: "0b5e3a8c1d2f4a6b8c9d0e1f2a3b4c5d0b5e3a8c1d2f4a6b8c9d0e1f2a3b4c5d".into(),
            entries: vec![
                entry("a.txt", 11, false),
                entry("d", 0, true),
                entry("d\\b.bin", 1 << 33, false),
            ],
            total_size: 11 + (1 << 33),
        };
        set(offer).expect("set");

        unsafe {
            OleInitialize(None).expect("OleInitialize");
            let clip: IDataObject = OleGetClipboard().expect("OleGetClipboard");
            let mut format = FORMATETC {
                cfFormat: descriptor_format(),
                ptd: std::ptr::null_mut(),
                dwAspect: DVASPECT_CONTENT.0,
                lindex: -1,
                tymed: TYMED_HGLOBAL.0 as u32,
            };
            let mut medium = clip.GetData(&format).expect("descriptor");
            let group = GlobalLock(medium.u.hGlobal) as *const FILEGROUPDESCRIPTORW;
            assert_eq!({ (*group).cItems }, 3);
            let first = std::ptr::addr_of!((*group).fgd) as *const FILEDESCRIPTORW;
            let third = std::ptr::read_unaligned(first.add(2));
            let name = third.cFileName;
            let len = name.iter().position(|&c| c == 0).unwrap();
            assert_eq!(String::from_utf16(&name[..len]).unwrap(), "d\\b.bin");
            assert_eq!({ third.nFileSizeHigh }, 2);
            assert_eq!({ third.nFileSizeLow }, 0);
            let folder = std::ptr::read_unaligned(first.add(1));
            assert_ne!({ folder.dwFileAttributes } & FILE_ATTRIBUTE_DIRECTORY, 0);
            let _ = GlobalUnlock(medium.u.hGlobal);
            ReleaseStgMedium(&mut medium);

            format.cfFormat = contents_format();
            format.tymed = TYMED_ISTREAM.0 as u32;
            // Explorer asks with GetData (the OLE clipboard's wrapper answers
            // QueryGetData from the format list alone).
            format.lindex = 1;
            assert!(clip.GetData(&format).is_err(), "a folder has no contents");
            format.lindex = 9;
            assert!(clip.GetData(&format).is_err(), "no such entry");

            format.lindex = 0;
            let mut medium = clip.GetData(&format).expect("stream");
            let stream = (*medium.u.pstm).clone().expect("an IStream");
            let mut stat = STATSTG::default();
            stream.Stat(&mut stat, STATFLAG_NONAME).expect("Stat");
            assert_eq!(stat.cbSize, 11);
            // Past the end is a position like any other; overflow and before-the-start are refused.
            let mut pos = 0u64;
            stream
                .Seek(20, STREAM_SEEK_SET, Some(&mut pos))
                .expect("past the end");
            assert_eq!(pos, 20);
            stream
                .Seek(-10, STREAM_SEEK_CUR, Some(&mut pos))
                .expect("back from past the end");
            assert_eq!(pos, 10);
            stream
                .Seek(-1, STREAM_SEEK_SET, Some(&mut pos))
                .expect("an absolute position is unsigned");
            assert_eq!(pos, u64::MAX);
            assert!(
                stream.Seek(1, STREAM_SEEK_CUR, Some(&mut pos)).is_err(),
                "overflow"
            );
            assert!(
                stream.Seek(-12, STREAM_SEEK_END, Some(&mut pos)).is_err(),
                "before the start"
            );
            drop(stream);
            ReleaseStgMedium(&mut medium);
            drop(clip);
        }

        clear_if_owned();
        std::thread::sleep(Duration::from_millis(200));
        unsafe {
            let clip = OleGetClipboard();
            let empty = clip.map_or(true, |c| {
                let format = FORMATETC {
                    cfFormat: descriptor_format(),
                    ptd: std::ptr::null_mut(),
                    dwAspect: DVASPECT_CONTENT.0,
                    lindex: -1,
                    tymed: TYMED_HGLOBAL.0 as u32,
                };
                c.GetData(&format).is_err()
            });
            assert!(empty, "clear takes the files off the clipboard");
        }
    }
}
