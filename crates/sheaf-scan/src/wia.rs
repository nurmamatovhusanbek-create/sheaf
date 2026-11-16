//! SCAN-WIA: Windows Image Acquisition 2 through COM.
//!
//! Not yet run against a real scanner (RISK-WIA): it type-checks for Windows
//! and enumerating devices is exercised in CI on a runner without one. Each
//! call runs on its own short-lived single-threaded-apartment thread, so no COM
//! object outlives the call and nothing COM crosses threads (SCAN-THREADING).
//! Driver variance is handled by trying to set each property and carrying on
//! when a driver refuses one it does not need.
#![allow(unsafe_code)]

use crate::{
    normalize, CancelToken, Device, ScanBackend, ScanColor, ScanPaper, ScanProfile, ScanSource, ScannedPage,
    MAX_DEVICES, MAX_PAGE_BYTES,
};
use sheaf_core::{AppError, ErrorCode, Result};
use std::cell::{Cell, RefCell};
use std::sync::mpsc::Sender;
use windows::core::{implement, AsImpl, Interface, BSTR, GUID};
use windows::Win32::Devices::ImageAcquisition::*;
use windows::Win32::Foundation::{E_ABORT, E_OUTOFMEMORY, HGLOBAL};
use windows::Win32::System::Com::StructuredStorage::{
    CreateStreamOnHGlobal, PropVariantClear, PROPSPEC, PROPSPEC_0, PROPVARIANT, PRSPEC_PROPID,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemAlloc, CoUninitialize, IStream, CLSCTX_LOCAL_SERVER,
    COINIT_APARTMENTTHREADED, STATFLAG_NONAME, STATSTG, STREAM_SEEK_SET,
};
use windows::Win32::System::Variant::{VT_BSTR, VT_CLSID, VT_I4};

// wiadef.h image formats (DEFINE_GUID, so the bindings do not carry them).
const FMT_BMP: GUID = GUID::from_u128(0xb96b3cab_0728_11d3_9d7b_0000f81ef32e);
const FMT_JPEG: GUID = GUID::from_u128(0xb96b3cae_0728_11d3_9d7b_0000f81ef32e);
const FMT_PNG: GUID = GUID::from_u128(0xb96b3caf_0728_11d3_9d7b_0000f81ef32e);

/// Scanners through WIA 2.
#[derive(Debug, Default, Clone, Copy)]
pub struct WiaBackend;

impl ScanBackend for WiaBackend {
    fn devices(&self) -> Result<Vec<Device>> {
        on_sta(|| unsafe { enumerate() })
    }

    fn scan(&self, profile: &ScanProfile, pages: Sender<ScannedPage>, cancel: CancelToken) -> Result<()> {
        profile.validate()?;
        let profile = profile.clone();
        on_sta(move || unsafe { scan_pages(&profile, pages, cancel) })
    }
}

/// Run `f` on a new thread that is a COM single-threaded apartment.
fn on_sta<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    let handle = std::thread::Builder::new()
        .name("wia".into())
        .spawn(move || {
            // SAFETY: balanced with CoUninitialize below, after `f` has dropped every COM object it made.
            let init = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
            if init.is_err() {
                return Err(AppError::new(
                    ErrorCode::DeviceError,
                    "could not start the scanner service",
                ));
            }
            let out = f();
            unsafe { CoUninitialize() };
            out
        })
        .map_err(|e| AppError::io("start the scanner thread", &e))?;
    handle.join().unwrap_or_else(|_| {
        Err(AppError::new(
            ErrorCode::Internal,
            "the scanner thread stopped unexpectedly",
        ))
    })
}

fn device_error(msg: &str) -> AppError {
    AppError::new(ErrorCode::DeviceError, msg)
}

/// SCAN-FAILURES: a WIA status code as one of ours. The messages carry no document content.
fn map_hresult(hr: windows::core::HRESULT, got_pages: bool) -> Option<AppError> {
    match hr {
        h if h == WIA_ERROR_PAPER_EMPTY => {
            // The feeder running dry after at least one page is how a batch ends.
            (!got_pages).then(|| AppError::new(ErrorCode::NoPaper, "the feeder is empty"))
        }
        h if h == WIA_ERROR_BUSY => Some(AppError::new(ErrorCode::DeviceBusy, "the scanner is busy")),
        h if h == WIA_ERROR_PAPER_JAM => Some(device_error("paper jam")),
        h if h == WIA_ERROR_PAPER_PROBLEM => Some(device_error("paper problem")),
        h if h == WIA_ERROR_COVER_OPEN => Some(device_error("the scanner cover is open")),
        h if h == WIA_ERROR_OFFLINE => Some(device_error("the scanner is offline")),
        h if h == E_ABORT => Some(AppError::new(ErrorCode::Cancelled, "cancelled")),
        h => Some(device_error(&format!("scanner error {:#010x}", h.0 as u32))),
    }
}

unsafe fn dev_mgr() -> Result<IWiaDevMgr2> {
    CoCreateInstance(&WiaDevMgr2, None, CLSCTX_LOCAL_SERVER)
        .map_err(|_| device_error("the scanner service is not available"))
}

// -- properties ---------------------------------------------------------------

fn spec(id: u32) -> PROPSPEC {
    PROPSPEC {
        ulKind: PRSPEC_PROPID,
        Anonymous: PROPSPEC_0 { propid: id },
    }
}

/// A string property, when the driver gives one.
unsafe fn read_string(store: &IWiaPropertyStorage, id: u32) -> Option<String> {
    let specs = [spec(id)];
    let mut vars = [PROPVARIANT::default()];
    store.ReadMultiple(1, specs.as_ptr(), vars.as_mut_ptr()).ok()?;
    let inner = &*vars[0].Anonymous.Anonymous;
    let out = (inner.vt == VT_BSTR).then(|| inner.Anonymous.bstrVal.to_string());
    let _ = PropVariantClear(&mut vars[0]);
    out
}

unsafe fn write_i32(store: &IWiaPropertyStorage, id: u32, value: i32) -> windows::core::Result<()> {
    let specs = [spec(id)];
    let mut var = PROPVARIANT::default();
    let inner = &mut *var.Anonymous.Anonymous;
    inner.vt = VT_I4;
    inner.Anonymous.lVal = value;
    store.WriteMultiple(1, specs.as_ptr(), &var, WIA_IPA_FIRST)
}

/// A `VT_CLSID` value that owns its GUID. `PropVariantClear`, which `PROPVARIANT`'s
/// `Drop` calls, frees the GUID with `CoTaskMemFree`: a pointer to a local variable
/// there (what this code used to do, on every scan) is freeing the stack, which
/// Windows answers by ending the process on the spot, with no panic and no message.
unsafe fn guid_variant(value: GUID) -> windows::core::Result<PROPVARIANT> {
    let memory = CoTaskMemAlloc(std::mem::size_of::<GUID>()).cast::<GUID>();
    if memory.is_null() {
        return Err(E_OUTOFMEMORY.into());
    }
    memory.write(value);
    let mut var = PROPVARIANT::default();
    let inner = &mut *var.Anonymous.Anonymous;
    inner.vt = VT_CLSID;
    inner.Anonymous.puuid = memory;
    Ok(var)
}

unsafe fn write_guid(store: &IWiaPropertyStorage, id: u32, value: GUID) -> windows::core::Result<()> {
    let specs = [spec(id)];
    let var = guid_variant(value)?;
    store.WriteMultiple(1, specs.as_ptr(), &var, WIA_IPA_FIRST)
}

// -- enumeration --------------------------------------------------------------

unsafe fn enumerate() -> Result<Vec<Device>> {
    let mgr = dev_mgr()?;
    // Local devices only: the app opens no network connections (SEC-NONET).
    let list = mgr
        .EnumDeviceInfo(WIA_DEVINFO_ENUM_LOCAL as i32)
        .map_err(|_| device_error("could not list scanners"))?;
    let mut out = Vec::new();
    while out.len() < MAX_DEVICES {
        let mut item: Option<IWiaPropertyStorage> = None;
        let mut got = 0u32;
        if list.Next(1, &mut item, &mut got).is_err() || got == 0 {
            break;
        }
        let Some(store) = item else { break };
        if let Some(id) = read_string(&store, WIA_DIP_DEV_ID) {
            let name = read_string(&store, WIA_DIP_DEV_NAME).unwrap_or_else(|| id.clone());
            out.push(Device { id, name });
        }
    }
    Ok(out)
}

// -- scanning -----------------------------------------------------------------

/// The item to scan from: the feeder or flatbed child when the device has them,
/// else the first image item.
unsafe fn scan_item(root: &IWiaItem2, source: ScanSource) -> Result<IWiaItem2> {
    let category = match source {
        ScanSource::Feeder => WIA_CATEGORY_FEEDER,
        ScanSource::Flatbed => WIA_CATEGORY_FLATBED,
    };
    let first = |list: windows::core::Result<IEnumWiaItem2>, only_images: bool| -> Option<IWiaItem2> {
        let list = list.ok()?;
        loop {
            let mut item: Option<IWiaItem2> = None;
            let mut got = 0u32;
            if list.Next(1, &mut item, &mut got).is_err() || got == 0 {
                return None;
            }
            let item = item?;
            if !only_images || item.GetItemType().is_ok_and(|t| t as u32 & WiaItemTypeImage != 0) {
                return Some(item);
            }
        }
    };
    first(root.EnumChildItems(Some(&category)), false)
        .or_else(|| first(root.EnumChildItems(None), true))
        .ok_or_else(|| device_error("the scanner has no scanning source"))
}

fn page_size_code(paper: ScanPaper) -> u32 {
    match paper {
        ScanPaper::A4 => WIA_PAGE_A4,
        ScanPaper::A3 => WIA_PAGE_ISO_A3,
        ScanPaper::Letter => WIA_PAGE_USLETTER,
        ScanPaper::Auto => WIA_PAGE_AUTO,
    }
}

/// Set what the profile asks for. A driver that refuses a property it does not
/// need is not an error; one that cannot do what was asked (a feeder) is.
unsafe fn configure(root: &IWiaItem2, item: &IWiaItem2, profile: &ScanProfile) -> Result<()> {
    let root_props = root.cast::<IWiaPropertyStorage>().ok();
    let item_props = item
        .cast::<IWiaPropertyStorage>()
        .map_err(|_| device_error("the scanner gave no settings"))?;

    let mut handling = match profile.source {
        ScanSource::Feeder => FEEDER,
        ScanSource::Flatbed => FLATBED,
    };
    if profile.duplex {
        handling |= DUPLEX;
    }
    // The setting lives on the item for WIA 2 drivers and on the device for older ones.
    let set_handling = write_i32(&item_props, WIA_DPS_DOCUMENT_HANDLING_SELECT, handling as i32).is_ok()
        | root_props
            .as_ref()
            .is_some_and(|r| write_i32(r, WIA_DPS_DOCUMENT_HANDLING_SELECT, handling as i32).is_ok());
    if !set_handling && (profile.duplex || profile.source == ScanSource::Feeder) {
        // A flatbed-only device refuses this; a feeder cannot be reached without it.
        let has_feeder_item = root.EnumChildItems(Some(&WIA_CATEGORY_FEEDER)).is_ok();
        if profile.duplex || !has_feeder_item {
            return Err(device_error(
                "the scanner cannot do that (feeder or two-sided scanning)",
            ));
        }
    }
    let pages = if profile.source == ScanSource::Feeder { 0 } else { 1 };
    for props in std::iter::once(&item_props).chain(root_props.as_ref()) {
        let _ = write_i32(props, WIA_DPS_PAGES, pages);
        let _ = write_i32(props, WIA_DPS_PAGE_SIZE, page_size_code(profile.paper) as i32);
    }

    let datatype = match profile.color {
        ScanColor::Color => WIA_DATA_COLOR,
        ScanColor::Gray => WIA_DATA_GRAYSCALE,
        ScanColor::Bw => WIA_DATA_THRESHOLD,
    };
    let _ = write_i32(&item_props, WIA_IPA_DATATYPE, datatype as i32);
    let dpi = profile.dpi as i32;
    let _ = write_i32(&item_props, WIA_IPS_XRES, dpi);
    let _ = write_i32(&item_props, WIA_IPS_YRES, dpi);
    if let Some((w, h)) = profile.pixels() {
        let _ = write_i32(&item_props, WIA_IPS_XPOS, 0);
        let _ = write_i32(&item_props, WIA_IPS_YPOS, 0);
        let _ = write_i32(&item_props, WIA_IPS_XEXTENT, w as i32);
        let _ = write_i32(&item_props, WIA_IPS_YEXTENT, h as i32);
    }
    // PNG, then JPEG, then BMP: the first one this driver accepts. TIFF is left
    // out on purpose (a multi-page TIFF would lose pages in `normalize`).
    let picked = [FMT_PNG, FMT_JPEG, FMT_BMP]
        .into_iter()
        .any(|f| write_guid(&item_props, WIA_IPA_FORMAT, f).is_ok());
    if !picked {
        return Err(device_error("the scanner offers no image format Sheaf can read"));
    }
    Ok(())
}

/// Receives the transfer: a stream for each page, then the page's bytes when it ends.
#[implement(IWiaTransferCallback)]
struct Collector {
    pages: Sender<ScannedPage>,
    cancel: CancelToken,
    current: RefCell<Option<IStream>>,
    sent: Cell<usize>,
    /// What went wrong inside the callback, which cannot return our error type.
    failure: RefCell<Option<AppError>>,
}

impl Collector {
    fn fail(&self, e: AppError) -> windows::core::Error {
        self.failure.borrow_mut().get_or_insert(e);
        windows::core::Error::from(E_ABORT)
    }

    fn finish_page(&self) -> Result<()> {
        let Some(stream) = self.current.borrow_mut().take() else {
            return Ok(());
        };
        let data = unsafe { drain(&stream)? };
        let (data, ext) = normalize(data)?;
        let number = self.sent.get() + 1;
        self.pages
            .send(ScannedPage { number, data, ext })
            .map_err(|_| AppError::new(ErrorCode::Cancelled, "nobody is listening"))?;
        self.sent.set(number);
        Ok(())
    }
}

/// All the bytes of a memory stream, within the page limit.
unsafe fn drain(stream: &IStream) -> Result<Vec<u8>> {
    let bad = || device_error("could not read the scanned page");
    let mut stat = STATSTG::default();
    stream.Stat(&mut stat, STATFLAG_NONAME).map_err(|_| bad())?;
    let size = usize::try_from(stat.cbSize).map_err(|_| bad())?;
    if size == 0 || size > MAX_PAGE_BYTES {
        return Err(bad());
    }
    stream.Seek(0, STREAM_SEEK_SET, None).map_err(|_| bad())?;
    let mut buf = vec![0u8; size];
    let mut read = 0u32;
    stream
        .Read(buf.as_mut_ptr().cast(), size as u32, Some(&mut read))
        .ok()
        .map_err(|_| bad())?;
    buf.truncate(read as usize);
    Ok(buf)
}

impl IWiaTransferCallback_Impl for Collector_Impl {
    fn TransferCallback(&self, _flags: i32, params: *const WiaTransferParams) -> windows::core::Result<()> {
        if self.cancel.is_cancelled() {
            return Err(windows::core::Error::from(E_ABORT));
        }
        // SAFETY: WIA passes a valid pointer for the duration of the call.
        let Some(params) = (unsafe { params.as_ref() }) else {
            return Ok(());
        };
        if params.hrErrorStatus.is_err() {
            let e =
                map_hresult(params.hrErrorStatus, self.sent.get() > 0).unwrap_or_else(|| device_error("scanner error"));
            return Err(self.fail(e));
        }
        if params.lMessage as u32 == WIA_TRANSFER_MSG_END_OF_STREAM {
            if let Err(e) = self.finish_page() {
                return Err(self.fail(e));
            }
        }
        Ok(())
    }

    fn GetNextStream(&self, _flags: i32, _name: &BSTR, _full_name: &BSTR) -> windows::core::Result<IStream> {
        if self.cancel.is_cancelled() {
            return Err(windows::core::Error::from(E_ABORT));
        }
        let stream = unsafe { CreateStreamOnHGlobal(HGLOBAL::default(), true) }?;
        *self.current.borrow_mut() = Some(stream.clone());
        Ok(stream)
    }
}

unsafe fn scan_pages(profile: &ScanProfile, pages: Sender<ScannedPage>, cancel: CancelToken) -> Result<()> {
    if cancel.is_cancelled() {
        return Err(AppError::new(ErrorCode::Cancelled, "cancelled"));
    }
    let mgr = dev_mgr()?;
    let root = mgr
        .CreateDevice(0, &BSTR::from(profile.device_id.as_str()))
        .map_err(|e| map_hresult(e.code(), false).unwrap_or_else(|| device_error("the scanner is not connected")))?;
    let item = scan_item(&root, profile.source)?;
    configure(&root, &item, profile)?;
    let transfer = item
        .cast::<IWiaTransfer>()
        .map_err(|_| device_error("the scanner cannot transfer pages"))?;

    let collector = Collector {
        pages,
        cancel: cancel.clone(),
        current: RefCell::new(None),
        sent: Cell::new(0),
        failure: RefCell::new(None),
    };
    let callback: IWiaTransferCallback = collector.into();
    let result = transfer.Download(0, &callback);
    // Take the facts back out of the callback object.
    let (sent, failure) = {
        let c: &Collector = callback.as_impl();
        (c.sent.get(), c.failure.borrow_mut().take())
    };
    if cancel.is_cancelled() {
        return Err(AppError::new(ErrorCode::Cancelled, "cancelled"));
    }
    if let Some(e) = failure {
        return Err(e);
    }
    match result {
        Ok(()) => Ok(()),
        Err(e) => match map_hresult(e.code(), sent > 0) {
            Some(err) => Err(err),
            None => Ok(()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs on a CI runner with no scanner: COM starts, the WIA service answers and the list is empty or not.
    #[test]
    fn listing_devices_works_without_a_scanner() {
        match WiaBackend.devices() {
            Ok(list) => assert!(list.len() <= MAX_DEVICES),
            // A runner without the WIA service says so in our own words, not with a crash.
            Err(e) => assert_eq!(e.code, ErrorCode::DeviceError, "{e}"),
        }
    }

    #[test]
    fn a_guid_property_owns_its_memory_so_dropping_it_is_safe() {
        // The variant is cleared (and the GUID freed) when it goes out of scope.
        // Before the fix the GUID lived on the stack and this was a crash.
        for format in [FMT_PNG, FMT_JPEG, FMT_BMP] {
            let var = unsafe { guid_variant(format) }.expect("variant");
            // SAFETY: the variant was just built as VT_CLSID with a valid pointer.
            let stored = unsafe { *var.Anonymous.Anonymous.Anonymous.puuid };
            assert_eq!(stored, format);
            drop(var);
        }
    }

    #[test]
    fn statuses_map_to_the_failure_codes() {
        assert_eq!(
            map_hresult(WIA_ERROR_PAPER_EMPTY, false).map(|e| e.code),
            Some(ErrorCode::NoPaper)
        );
        assert!(
            map_hresult(WIA_ERROR_PAPER_EMPTY, true).is_none(),
            "the end of a batch is not an error"
        );
        assert_eq!(
            map_hresult(WIA_ERROR_BUSY, true).map(|e| e.code),
            Some(ErrorCode::DeviceBusy)
        );
        assert_eq!(
            map_hresult(WIA_ERROR_PAPER_JAM, true).map(|e| e.code),
            Some(ErrorCode::DeviceError)
        );
        assert_eq!(map_hresult(E_ABORT, true).map(|e| e.code), Some(ErrorCode::Cancelled));
    }
}
