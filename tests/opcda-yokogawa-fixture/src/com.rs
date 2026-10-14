#![allow(
    non_snake_case,
    clippy::too_many_arguments,
    reason = "Windows and OPC DA COM interfaces prescribe these method signatures"
)]

use std::{
    collections::HashMap,
    ffi::c_void,
    fs::OpenOptions,
    mem::{ManuallyDrop, size_of},
    ptr,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use windows::Win32::{
    Foundation::{
        CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_FAIL, E_INVALIDARG, E_NOTIMPL,
        E_OUTOFMEMORY, E_POINTER, E_UNEXPECTED, FILETIME, S_FALSE, S_OK, SysAllocStringLen,
    },
    System::{
        Com::{
            CoTaskMemAlloc, CoTaskMemFree, IClassFactory, IClassFactory_Impl, IConnectionPoint,
            IConnectionPointContainer, IConnectionPointContainer_Impl, IEnumConnectionPoints,
            IEnumGUID, IEnumGUID_Impl, IEnumString, IEnumString_Impl,
        },
        Variant::{VARIANT, VT_BSTR, VT_EMPTY, VT_I2, VT_I4, VT_R4, VT_R8, VariantClear},
    },
};
use windows_core::{
    BOOL, Error, GUID, HRESULT, IUnknown, Interface, PCWSTR, PWSTR, Ref, Result, implement,
};

use crate::{
    abi::*,
    plant::{BRANCHES, PREFIX, Plant, TAGS, Value, WriteError, tag_index, writable},
};

const SERVER_CLASS: GUID = GUID::from_u128(0x0e444026_a6c9_4aaf_ba49_1203649745b1);
const CATALOG_CLASS: GUID = GUID::from_u128(0x65cf766f_dfdc_423e_9a67_e6fd1c1ab4f3);
const DA2_CATEGORY: GUID = GUID::from_u128(0x63d5f432_cfe4_11d1_b2c8_0060083ba1fb);
const PROG_ID: &str = "ByteHound.BHTuneOpcDaSmoke.1";
const INVALID_HANDLE: HRESULT = HRESULT(0xc0040001_u32 as i32);
const BAD_TYPE: HRESULT = HRESULT(0xc0040004_u32 as i32);
const BAD_RIGHTS: HRESULT = HRESULT(0xc0040006_u32 as i32);
const UNKNOWN_ITEM: HRESULT = HRESULT(0xc0040007_u32 as i32);
const INVALID_VALUE: HRESULT = HRESULT(0xc004000b_u32 as i32);
const MAX_ITEMS: u32 = 1024;

struct SharedPlant {
    plant: Mutex<Plant>,
    started: Instant,
}

static SHARED: Mutex<Option<Arc<SharedPlant>>> = Mutex::new(None);

fn lock<T>(mutex: &Mutex<T>) -> Result<MutexGuard<'_, T>> {
    mutex.lock().map_err(|error| {
        eprintln!("OPC DA fixture mutex poisoned: {error}");
        Error::from_hresult(E_UNEXPECTED)
    })
}

fn shared_plant() -> Result<Arc<SharedPlant>> {
    let mut slot = lock(&SHARED)?;
    if let Some(plant) = slot.as_ref() {
        return Ok(Arc::clone(plant));
    }
    let path = std::env::var_os("BHTUNE_OPCDA_FIXTURE_AUDIT").ok_or_else(|| {
        eprintln!("BHTUNE_OPCDA_FIXTURE_AUDIT is required");
        Error::from_hresult(E_UNEXPECTED)
    })?;
    let audit = OpenOptions::new()
        .append(true)
        .open(path)
        .map_err(|error| {
            eprintln!("Cannot open OPC DA fixture audit: {error}");
            Error::from_hresult(E_UNEXPECTED)
        })?;
    let plant = Arc::new(SharedPlant {
        plant: Mutex::new(Plant::new(audit)),
        started: Instant::now(),
    });
    *slot = Some(Arc::clone(&plant));
    Ok(plant)
}

fn code(result: Result<HRESULT>) -> HRESULT {
    match result {
        Ok(code) => code,
        Err(error) => error.code(),
    }
}

fn batch_code(errors: &[HRESULT]) -> HRESULT {
    if errors.iter().any(|error| error.is_err()) {
        S_FALSE
    } else {
        S_OK
    }
}

unsafe fn input<'a, T>(pointer: *const T, count: u32) -> Result<&'a [T]> {
    if count > MAX_ITEMS || (count != 0 && pointer.is_null()) {
        return Err(Error::from_hresult(E_INVALIDARG));
    }
    if count == 0 {
        return Ok(&[]);
    }
    // SAFETY: COM callers supply an aligned live input array of count elements.
    Ok(unsafe { std::slice::from_raw_parts(pointer, count as usize) })
}

unsafe fn output<T>(pointer: *mut T, value: T) -> Result<()> {
    if pointer.is_null() {
        return Err(Error::from_hresult(E_POINTER));
    }
    // SAFETY: COM callers supply writable storage for this output parameter.
    unsafe { pointer.write(value) };
    Ok(())
}

unsafe fn wide(value: PCWSTR) -> Result<String> {
    if value.is_null() {
        return Err(Error::from_hresult(E_POINTER));
    }
    for length in 0..4096 {
        // SAFETY: COM string inputs are live NUL-terminated UTF-16 buffers.
        if unsafe { *value.0.add(length) } == 0 {
            // SAFETY: These units precede the terminator in that same buffer.
            let units = unsafe { std::slice::from_raw_parts(value.0, length) };
            return String::from_utf16(units).map_err(|_| Error::from_hresult(E_INVALIDARG));
        }
    }
    Err(Error::from_hresult(E_INVALIDARG))
}

struct TaskArray<T> {
    pointer: *mut T,
    length: usize,
}

impl<T> TaskArray<T> {
    fn new(values: Vec<T>) -> Result<Self> {
        let bytes = values
            .len()
            .checked_mul(size_of::<T>())
            .ok_or_else(|| Error::from_hresult(E_OUTOFMEMORY))?;
        // SAFETY: The allocation is sized for exactly these values, with COM heap alignment.
        let pointer = unsafe { CoTaskMemAlloc(bytes) }.cast::<T>();
        if pointer.is_null() {
            return Err(Error::from_hresult(E_OUTOFMEMORY));
        }
        let length = values.len();
        for (index, value) in values.into_iter().enumerate() {
            // SAFETY: Each allocated slot is initialized exactly once by moving its value.
            unsafe { pointer.add(index).write(value) };
        }
        Ok(Self { pointer, length })
    }

    fn release(self) -> *mut T {
        let pointer = self.pointer;
        std::mem::forget(self);
        pointer
    }
}

impl<T> Drop for TaskArray<T> {
    fn drop(&mut self) {
        // SAFETY: All slots are initialized and still owned here; release transfers ownership.
        unsafe {
            ptr::drop_in_place(ptr::slice_from_raw_parts_mut(self.pointer, self.length));
            CoTaskMemFree(Some(self.pointer.cast()));
        }
    }
}

fn task_string(text: &str) -> Result<TaskArray<u16>> {
    TaskArray::new(text.encode_utf16().chain([0]).collect())
}

impl Drop for ItemState {
    fn drop(&mut self) {
        // SAFETY: Fixture item states contain an initialized, uniquely owned VARIANT.
        if let Err(error) = unsafe { VariantClear(&mut self.value) } {
            eprintln!("Cannot release fixture VARIANT: {error}");
        }
    }
}

fn to_variant(value: Value) -> Result<VARIANT> {
    let mut result = VARIANT::default();
    match value {
        Value::Number(number) => {
            // SAFETY: The discriminant and matching numeric union member are set together.
            unsafe {
                let inner = &mut *result.Anonymous.Anonymous;
                inner.vt = VT_R4;
                inner.Anonymous.fltVal = number;
            }
        }
        Value::Text(text) => {
            let units = text.encode_utf16().collect::<Vec<_>>();
            // SAFETY: SysAllocStringLen copies the live UTF-16 slice into a new BSTR.
            let string = unsafe { SysAllocStringLen(Some(&units)) };
            if string.is_empty() && !units.is_empty() {
                return Err(Error::from_hresult(E_OUTOFMEMORY));
            }
            // SAFETY: This VARIANT receives unique BSTR ownership and the matching type tag.
            unsafe {
                let inner = &mut *result.Anonymous.Anonymous;
                inner.vt = VT_BSTR;
                inner.Anonymous.bstrVal = ManuallyDrop::new(string);
            }
        }
    }
    Ok(result)
}

fn from_variant(value: &VARIANT) -> Result<Value> {
    // SAFETY: The COM caller provides a live VARIANT; only the tagged member is accessed.
    unsafe {
        let inner = &value.Anonymous.Anonymous;
        match inner.vt {
            VT_R4 => Ok(Value::Number(inner.Anonymous.fltVal)),
            VT_R8 => Ok(Value::Number(inner.Anonymous.dblVal as f32)),
            VT_I2 => Ok(Value::Number(f32::from(inner.Anonymous.iVal))),
            VT_I4 => Ok(Value::Number(inner.Anonymous.lVal as f32)),
            VT_BSTR => String::from_utf16(&inner.Anonymous.bstrVal)
                .map(Value::Text)
                .map_err(|_| Error::from_hresult(BAD_TYPE)),
            _ => Err(Error::from_hresult(BAD_TYPE)),
        }
    }
}

fn timestamp() -> Result<FILETIME> {
    let unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| {
            eprintln!("Cannot timestamp fixture data: {error}");
            Error::from_hresult(E_UNEXPECTED)
        })?;
    let ticks = u64::try_from(unix.as_nanos() / 100)
        .ok()
        .and_then(|ticks| ticks.checked_add(116_444_736_000_000_000))
        .ok_or_else(|| Error::from_hresult(E_UNEXPECTED))?;
    Ok(FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    })
}

#[implement(IClassFactory)]
struct Factory {
    catalog: bool,
}

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(
        &self,
        outer: Ref<'_, IUnknown>,
        iid: *const GUID,
        object: *mut *mut c_void,
    ) -> Result<()> {
        if outer.as_ref().is_some() {
            return Err(Error::from_hresult(CLASS_E_NOAGGREGATION));
        }
        if iid.is_null() {
            return Err(Error::from_hresult(E_POINTER));
        }
        // SAFETY: The class-factory contract supplies a writable interface output.
        unsafe { output(object, ptr::null_mut())? };
        let unknown: IUnknown = if self.catalog {
            Catalog.into()
        } else {
            Server::new(shared_plant()?).into()
        };
        // SAFETY: IID/output are validated, and the local object owns the queried interface.
        unsafe { unknown.query(iid, object).ok() }
    }

    fn LockServer(&self, _lock: BOOL) -> Result<()> {
        Ok(())
    }
}

/// # Safety
/// Windows supplies live CLSID/IID pointers and writable interface output storage.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllGetClassObject(
    class: *const GUID,
    iid: *const GUID,
    object: *mut *mut c_void,
) -> HRESULT {
    code((|| {
        if class.is_null() || iid.is_null() {
            return Err(Error::from_hresult(E_POINTER));
        }
        // SAFETY: Windows supplies GUID inputs and writable output storage for this export.
        unsafe { output(object, ptr::null_mut())? };
        // SAFETY: The non-null CLSID refers to a live GUID supplied by Windows.
        let class = unsafe { *class };
        if class != SERVER_CLASS && class != CATALOG_CLASS {
            return Err(Error::from_hresult(CLASS_E_CLASSNOTAVAILABLE));
        }
        let factory: IClassFactory = Factory {
            catalog: class == CATALOG_CLASS,
        }
        .into();
        // SAFETY: Windows supplies the live IID and validated interface output.
        unsafe { factory.query(iid, object).ok()? };
        Ok(S_OK)
    })())
}

#[unsafe(no_mangle)]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    // ponytail: retain the plant for one bounded gateway child; no reusable-service unload policy.
    S_FALSE
}

#[implement(
    IOpcServer,
    IOpcCommon,
    IOpcBrowse,
    IOpcItemProperties,
    IConnectionPointContainer
)]
struct Server {
    shared: Arc<SharedPlant>,
    cursor: Mutex<usize>,
    locale: AtomicU32,
    next_group: AtomicU32,
    groups: Mutex<HashMap<u32, Arc<GroupState>>>,
}

impl Server {
    fn new(shared: Arc<SharedPlant>) -> Self {
        Self {
            shared,
            cursor: Mutex::new(0),
            locale: AtomicU32::new(1033),
            next_group: AtomicU32::new(1),
            groups: Mutex::new(HashMap::new()),
        }
    }
}

impl IOpcServer_Impl for Server_Impl {
    unsafe fn add_group(
        &self,
        _name: PCWSTR,
        _active: BOOL,
        rate: u32,
        _client: u32,
        _bias: *const i32,
        _deadband: *const f32,
        _locale: u32,
        handle: *mut u32,
        revised: *mut u32,
        iid: *const GUID,
        object: *mut *mut c_void,
    ) -> HRESULT {
        code((|| {
            if iid.is_null() || handle.is_null() || revised.is_null() {
                return Err(Error::from_hresult(E_POINTER));
            }
            // SAFETY: The OPC DA AddGroup contract provides these scalar/interface outputs.
            unsafe { output(object, ptr::null_mut())? };
            let mut groups = lock(&self.groups)?;
            if groups.len() >= 64 {
                return Err(Error::from_hresult(E_OUTOFMEMORY));
            }
            let group_handle = self.next_group.fetch_add(1, Ordering::Relaxed);
            let state = Arc::new(GroupState::default());
            let group: IOpcItemMgt = Group {
                shared: Arc::clone(&self.shared),
                state: Arc::clone(&state),
            }
            .into();
            // SAFETY: The IID and output pointers are valid under the AddGroup ABI contract.
            unsafe {
                group.query(iid, object).ok()?;
                output(handle, group_handle)?;
                output(revised, rate)?;
            }
            groups.insert(group_handle, state);
            Ok(S_OK)
        })())
    }

    unsafe fn get_error_string(&self, error: HRESULT, _locale: u32, text: *mut PWSTR) -> HRESULT {
        // SAFETY: The caller supplies one writable PWSTR output.
        unsafe { IOpcCommon_Impl::get_error_string(self, error, text) }
    }

    unsafe fn get_status(&self, status: *mut *mut ServerStatus) -> HRESULT {
        code((|| {
            // SAFETY: The caller supplies the status-pointer output required by OPC DA.
            unsafe { output(status, ptr::null_mut())? };
            let now = timestamp()?;
            let vendor = task_string("BHTune plant-free OPC DA fixture")?;
            let data = TaskArray::new(vec![ServerStatus {
                start: now,
                current: now,
                updated: now,
                state: 1,
                group_count: lock(&self.groups)?.len() as u32,
                bandwidth: 0,
                major: 2,
                minor: 0,
                build: 1,
                reserved: 0,
                vendor: PWSTR(vendor.pointer),
            }])?;
            vendor.release();
            // SAFETY: Both COM allocations are transferred to the caller through its output.
            unsafe { output(status, data.release())? };
            Ok(S_OK)
        })())
    }

    unsafe fn remove_group(&self, handle: u32, _force: BOOL) -> HRESULT {
        code((|| {
            let state = lock(&self.groups)?
                .remove(&handle)
                .ok_or_else(|| Error::from_hresult(INVALID_HANDLE))?;
            state.removed.store(true, Ordering::Release);
            Ok(S_OK)
        })())
    }

    unsupported! {
        fn get_group_by_name(_name: PCWSTR, _iid: *const GUID, _object: *mut *mut c_void);
        fn create_group_enumerator(_scope: i32, _iid: *const GUID, _object: *mut *mut c_void);
    }
}

impl IOpcCommon_Impl for Server_Impl {
    unsafe fn set_locale(&self, locale: u32) -> HRESULT {
        if !matches!(locale, 0 | 1033) {
            return E_INVALIDARG;
        }
        self.locale.store(locale, Ordering::Relaxed);
        S_OK
    }

    unsafe fn get_locale(&self, locale: *mut u32) -> HRESULT {
        // SAFETY: The caller supplies a writable locale output.
        code(unsafe { output(locale, self.locale.load(Ordering::Relaxed)) }.map(|()| S_OK))
    }

    unsafe fn query_locales(&self, count: *mut u32, locales: *mut *mut u32) -> HRESULT {
        code((|| {
            if count.is_null() {
                return Err(Error::from_hresult(E_POINTER));
            }
            // SAFETY: The caller supplies the scalar and array outputs defined by IOPCCommon.
            unsafe { output(locales, ptr::null_mut())? };
            let values = TaskArray::new(vec![1033])?;
            // SAFETY: The allocation is handed to the caller through its live outputs.
            unsafe {
                output(count, 1)?;
                output(locales, values.release())?;
            }
            Ok(S_OK)
        })())
    }

    unsafe fn get_error_string(&self, error: HRESULT, text: *mut PWSTR) -> HRESULT {
        code((|| {
            // SAFETY: The caller supplies the PWSTR output required by this COM method.
            unsafe { output(text, PWSTR::null())? };
            let value = task_string(&format!("OPC DA fixture HRESULT {:#010x}", error.0))?;
            // SAFETY: The newly allocated COM string is transferred through that output.
            unsafe { output(text, PWSTR(value.release()))? };
            Ok(S_OK)
        })())
    }

    unsafe fn set_client_name(&self, name: PCWSTR) -> HRESULT {
        // SAFETY: IOPCCommon provides a live NUL-terminated name; validate it before accepting.
        code(unsafe { wide(name) }.map(|_| S_OK))
    }
}

impl IOpcBrowse_Impl for Server_Impl {
    unsafe fn query_organization(&self, organization: *mut i32) -> HRESULT {
        // SAFETY: The caller supplies the organization output.
        code(unsafe { output(organization, 1) }.map(|()| S_OK))
    }

    unsafe fn change_position(&self, direction: i32, name: PCWSTR) -> HRESULT {
        code((|| {
            // SAFETY: The browse-position contract provides a live UTF-16 input.
            let name = unsafe { wide(name)? };
            let mut cursor = lock(&self.cursor)?;
            match direction {
                1 if *cursor > 0 => *cursor -= 1,
                1 => return Err(Error::from_hresult(E_FAIL)),
                2 if BRANCHES.get(*cursor).is_some_and(|branch| *branch == name) => *cursor += 1,
                3 if name.is_empty() => *cursor = 0,
                _ => return Err(Error::from_hresult(E_INVALIDARG)),
            }
            Ok(S_OK)
        })())
    }

    unsafe fn browse_ids(
        &self,
        filter: i32,
        criteria: PCWSTR,
        datatype: u16,
        rights: u32,
        enumerator: *mut *mut c_void,
    ) -> HRESULT {
        code((|| {
            // SAFETY: The caller supplies a string input and writable COM interface output.
            let criteria = unsafe { wide(criteria)? };
            // SAFETY: The output slot is valid under BrowseOPCItemIDs' COM contract.
            unsafe { output(enumerator, ptr::null_mut())? };
            if (!criteria.is_empty() && criteria != "*") || datatype != 0 || rights != 0 {
                return Err(Error::from_hresult(E_NOTIMPL));
            }
            let cursor = *lock(&self.cursor)?;
            let names = match filter {
                1 => BRANCHES
                    .get(cursor)
                    .map(|name| vec![(*name).to_owned()])
                    .unwrap_or_default(),
                2 if cursor == BRANCHES.len() => {
                    TAGS.iter().map(|name| (*name).to_owned()).collect()
                }
                2 => vec![],
                3 => TAGS.iter().map(|name| format!("{PREFIX}.{name}")).collect(),
                _ => return Err(Error::from_hresult(E_INVALIDARG)),
            };
            let result: IEnumString = Strings {
                names: Arc::new(names),
                cursor: Mutex::new(0),
            }
            .into();
            // SAFETY: This transfers the new enumerator's reference to the caller's output.
            unsafe { output(enumerator, result.into_raw())? };
            Ok(S_OK)
        })())
    }

    unsafe fn get_item_id(&self, name: PCWSTR, item_id: *mut PWSTR) -> HRESULT {
        code((|| {
            // SAFETY: The caller supplies a live item name and a writable string output.
            let name = unsafe { wide(name)? };
            // SAFETY: Clear that validated output before any fallible resolution/allocation.
            unsafe { output(item_id, PWSTR::null())? };
            let id = if tag_index(&name).is_some() {
                name
            } else if *lock(&self.cursor)? == BRANCHES.len() && TAGS.contains(&name.as_str()) {
                format!("{PREFIX}.{name}")
            } else {
                // The native DA2 classifier requires E_INVALIDARG for pure branches.
                return Err(Error::from_hresult(E_INVALIDARG));
            };
            let text = task_string(&id)?;
            // SAFETY: Ownership of the resolved COM string transfers to the caller.
            unsafe { output(item_id, PWSTR(text.release()))? };
            Ok(S_OK)
        })())
    }

    unsupported! { fn browse_access_paths(_id: PCWSTR, _enumerator: *mut *mut c_void); }
}

#[implement(IEnumString)]
struct Strings {
    names: Arc<Vec<String>>,
    cursor: Mutex<usize>,
}

impl IEnumString_Impl for Strings_Impl {
    fn Next(&self, count: u32, strings: *mut PWSTR, fetched: *mut u32) -> HRESULT {
        code((|| {
            if strings.is_null()
                || count == 0
                || count > MAX_ITEMS
                || (fetched.is_null() && count != 1)
            {
                return Err(Error::from_hresult(E_INVALIDARG));
            }
            let mut cursor = lock(&self.cursor)?;
            let available = (count as usize).min(self.names.len() - *cursor);
            let values = self.names[*cursor..*cursor + available]
                .iter()
                .map(|name| task_string(name))
                .collect::<Result<Vec<_>>>()?;
            // SAFETY: IEnumString supplies count writable slots and the optional fetched output.
            unsafe {
                for (index, value) in values.into_iter().enumerate() {
                    output(strings.add(index), PWSTR(value.release()))?;
                }
                if !fetched.is_null() {
                    output(fetched, available as u32)?;
                }
            }
            *cursor += available;
            Ok(if available == count as usize {
                S_OK
            } else {
                S_FALSE
            })
        })())
    }

    fn Skip(&self, count: u32) -> HRESULT {
        code((|| {
            let mut cursor = lock(&self.cursor)?;
            let skipped = (count as usize).min(self.names.len() - *cursor);
            *cursor += skipped;
            Ok(if skipped == count as usize {
                S_OK
            } else {
                S_FALSE
            })
        })())
    }

    fn Reset(&self) -> Result<()> {
        *lock(&self.cursor)? = 0;
        Ok(())
    }

    fn Clone(&self) -> Result<IEnumString> {
        Ok(Strings {
            names: Arc::clone(&self.names),
            cursor: Mutex::new(*lock(&self.cursor)?),
        }
        .into())
    }
}

#[derive(Clone, Copy)]
struct Item {
    tag: usize,
    client: u32,
}

#[derive(Default)]
struct GroupState {
    items: Mutex<Vec<Option<Item>>>,
    removed: AtomicBool,
}

#[implement(
    IOpcItemMgt,
    IOpcGroupState,
    IOpcSyncIo,
    IOpcAsyncIo2,
    IConnectionPointContainer
)]
struct Group {
    shared: Arc<SharedPlant>,
    state: Arc<GroupState>,
}

impl Group {
    fn items(&self) -> Result<MutexGuard<'_, Vec<Option<Item>>>> {
        if self.state.removed.load(Ordering::Acquire) {
            return Err(Error::from_hresult(INVALID_HANDLE));
        }
        lock(&self.state.items)
    }
}

impl IOpcItemMgt_Impl for Group_Impl {
    unsafe fn add_items(
        &self,
        count: u32,
        definitions: *const ItemDef,
        results: *mut *mut ItemResult,
        errors: *mut *mut HRESULT,
    ) -> HRESULT {
        code((|| {
            if count == 0 {
                return Err(Error::from_hresult(E_INVALIDARG));
            }
            // SAFETY: AddItems' caller provides count ItemDef inputs and two array outputs.
            let definitions = unsafe { input(definitions, count)? };
            // SAFETY: The two live outputs are cleared before allocating their payloads.
            unsafe {
                output(results, ptr::null_mut())?;
                output(errors, ptr::null_mut())?;
            }
            let mut items = self.items()?;
            if items.len() + definitions.len() > MAX_ITEMS as usize {
                return Err(Error::from_hresult(E_OUTOFMEMORY));
            }
            let mut output_items = Vec::with_capacity(definitions.len());
            let mut output_errors = Vec::with_capacity(definitions.len());
            for definition in definitions {
                // SAFETY: Each ItemDef contains the live UTF-16 ItemID specified by OPC DA.
                let name = unsafe { wide(PCWSTR(definition.item_id.0))? };
                let tag = tag_index(&name);
                let canonical = if tag == Some(4) { VT_BSTR.0 } else { VT_R4.0 };
                let error = match tag {
                    None => UNKNOWN_ITEM,
                    Some(_)
                        if definition.requested_type != VT_EMPTY.0
                            && definition.requested_type != canonical =>
                    {
                        BAD_TYPE
                    }
                    Some(tag) => {
                        items.push(Some(Item {
                            tag,
                            client: definition.client_handle,
                        }));
                        S_OK
                    }
                };
                output_items.push(ItemResult {
                    server_handle: if error.is_ok() { items.len() as u32 } else { 0 },
                    canonical_type: canonical,
                    reserved: 0,
                    access_rights: if tag.is_some_and(writable) { 3 } else { 1 },
                    blob_size: 0,
                    blob: ptr::null_mut(),
                });
                output_errors.push(error);
            }
            let result = batch_code(&output_errors);
            let output_items = TaskArray::new(output_items)?;
            let output_errors = TaskArray::new(output_errors)?;
            // SAFETY: Transfer both completed COM allocations to their checked outputs.
            unsafe {
                output(results, output_items.release())?;
                output(errors, output_errors.release())?;
            }
            Ok(result)
        })())
    }

    unsafe fn remove_items(
        &self,
        count: u32,
        handles: *const u32,
        errors: *mut *mut HRESULT,
    ) -> HRESULT {
        code((|| {
            // SAFETY: RemoveItems provides a live handle array and its writable error output.
            let handles = unsafe { input(handles, count)? };
            // SAFETY: The COM output is valid and cleared before allocation.
            unsafe { output(errors, ptr::null_mut())? };
            let mut items = self.items()?;
            let mut values = Vec::with_capacity(handles.len());
            for handle in handles {
                let item = handle
                    .checked_sub(1)
                    .and_then(|index| items.get_mut(index as usize));
                values.push(if item.and_then(Option::take).is_some() {
                    S_OK
                } else {
                    INVALID_HANDLE
                });
            }
            let result = batch_code(&values);
            let values = TaskArray::new(values)?;
            // SAFETY: The caller receives ownership of the populated COM error array.
            unsafe { output(errors, values.release())? };
            Ok(result)
        })())
    }

    unsupported! {
        fn validate_items(_count: u32, _items: *const ItemDef, _update: BOOL, _results: *mut *mut ItemResult, _errors: *mut *mut HRESULT);
        fn set_active(_count: u32, _handles: *const u32, _active: BOOL, _errors: *mut *mut HRESULT);
        fn set_client_handles(_count: u32, _handles: *const u32, _clients: *const u32, _errors: *mut *mut HRESULT);
        fn set_datatypes(_count: u32, _handles: *const u32, _types: *const u16, _errors: *mut *mut HRESULT);
        fn create_enumerator(_iid: *const GUID, _object: *mut *mut c_void);
    }
}

impl IOpcSyncIo_Impl for Group_Impl {
    unsafe fn read(
        &self,
        source: i32,
        count: u32,
        handles: *const u32,
        values: *mut *mut ItemState,
        errors: *mut *mut HRESULT,
    ) -> HRESULT {
        code((|| {
            if count == 0 || !matches!(source, 1 | 2) {
                return Err(Error::from_hresult(E_INVALIDARG));
            }
            // SAFETY: The sync-read ABI provides count handles and two live array outputs.
            let handles = unsafe { input(handles, count)? };
            // SAFETY: Clear the caller's outputs before constructing their allocations.
            unsafe {
                output(values, ptr::null_mut())?;
                output(errors, ptr::null_mut())?;
            }
            let items = self.items()?;
            let mut plant = lock(&self.shared.plant)?;
            plant.advance(self.shared.started.elapsed());
            let timestamp = timestamp()?;
            let mut states = Vec::with_capacity(handles.len());
            let mut failures = Vec::with_capacity(handles.len());
            for handle in handles {
                let item = handle
                    .checked_sub(1)
                    .and_then(|index| items.get(index as usize))
                    .copied()
                    .flatten();
                let value = item.and_then(|item| plant.read(item.tag));
                let valid = value.is_some();
                states.push(ItemState {
                    client_handle: item.map_or(0, |item| item.client),
                    timestamp,
                    quality: if valid { 0xc0 } else { 0 },
                    reserved: 0,
                    value: match value {
                        Some(value) => to_variant(value)?,
                        None => VARIANT::default(),
                    },
                });
                failures.push(if valid { S_OK } else { INVALID_HANDLE });
            }
            let result = batch_code(&failures);
            let states = TaskArray::new(states)?;
            let failures = TaskArray::new(failures)?;
            // SAFETY: Transfer both complete allocations through the caller's checked outputs.
            unsafe {
                output(values, states.release())?;
                output(errors, failures.release())?;
            }
            Ok(result)
        })())
    }

    unsafe fn write(
        &self,
        count: u32,
        handles: *const u32,
        values: *const VARIANT,
        errors: *mut *mut HRESULT,
    ) -> HRESULT {
        code((|| {
            if count == 0 {
                return Err(Error::from_hresult(E_INVALIDARG));
            }
            // SAFETY: The sync-write caller provides count handles and matching live VARIANTs.
            let handles = unsafe { input(handles, count)? };
            // SAFETY: The VARIANT buffer has the same length under that OPC DA contract.
            let values = unsafe { input(values, count)? };
            // SAFETY: The caller provides the required error-array output.
            unsafe { output(errors, ptr::null_mut())? };
            let items = self.items()?;
            let mut plant = lock(&self.shared.plant)?;
            let elapsed = self.shared.started.elapsed();
            let mut failures = Vec::with_capacity(handles.len());
            for (handle, value) in handles.iter().zip(values) {
                let item = handle
                    .checked_sub(1)
                    .and_then(|index| items.get(index as usize))
                    .copied()
                    .flatten();
                let result = match item {
                    None => INVALID_HANDLE,
                    Some(item) => match from_variant(value) {
                        Err(error) => error.code(),
                        Ok(value) => match plant.write(item.tag, value, elapsed) {
                            Ok(()) => S_OK,
                            Err(WriteError::BadType) => BAD_TYPE,
                            Err(WriteError::ReadOnly | WriteError::ManualRequired) => BAD_RIGHTS,
                            Err(WriteError::InvalidValue) => INVALID_VALUE,
                            Err(WriteError::Audit(error)) => {
                                eprintln!("Fixture write audit failed: {error}");
                                E_FAIL
                            }
                        },
                    },
                };
                failures.push(result);
            }
            let result = batch_code(&failures);
            let failures = TaskArray::new(failures)?;
            // SAFETY: Transfer the populated per-item HRESULT array to the checked output.
            unsafe { output(errors, failures.release())? };
            Ok(result)
        })())
    }
}

macro_rules! unsupported {
    ($(fn $name:ident($($argument:ident: $type:ty),*);)*) => {
        $(unsafe fn $name(&self, $($argument: $type),*) -> HRESULT { E_NOTIMPL })*
    };
}
use unsupported;

impl IOpcItemProperties_Impl for Server_Impl {
    unsupported! {
        fn query_properties(_id: PCWSTR, _count: *mut u32, _ids: *mut *mut u32, _descriptions: *mut *mut PWSTR, _types: *mut *mut u16);
        fn get_properties(_id: PCWSTR, _count: u32, _ids: *const u32, _values: *mut *mut VARIANT, _errors: *mut *mut HRESULT);
        fn lookup_item_ids(_id: PCWSTR, _count: u32, _ids: *const u32, _values: *mut *mut PWSTR, _errors: *mut *mut HRESULT);
    }
}

impl IOpcGroupState_Impl for Group_Impl {
    unsupported! {
        fn get_state(_rate: *mut u32, _active: *mut BOOL, _name: *mut PWSTR, _bias: *mut i32, _deadband: *mut f32, _locale: *mut u32, _client: *mut u32, _server: *mut u32);
        fn set_state(_rate: *const u32, _revised: *mut u32, _active: *const BOOL, _bias: *const i32, _deadband: *const f32, _locale: *const u32, _client: *const u32);
        fn set_name(_name: PCWSTR);
        fn clone_group(_name: PCWSTR, _iid: *const GUID, _object: *mut *mut c_void);
    }
}

impl IOpcAsyncIo2_Impl for Group_Impl {
    unsupported! {
        fn read(_count: u32, _handles: *const u32, _transaction: u32, _cancel: *mut u32, _errors: *mut *mut HRESULT);
        fn write(_count: u32, _handles: *const u32, _values: *const VARIANT, _transaction: u32, _cancel: *mut u32, _errors: *mut *mut HRESULT);
        fn refresh(_source: i32, _transaction: u32, _cancel: *mut u32);
        fn cancel(_cancel: u32);
        fn set_enable(_enable: BOOL);
        fn get_enable(_enable: *mut BOOL);
    }
}

macro_rules! no_connection_points {
    ($type:ty) => {
        impl IConnectionPointContainer_Impl for $type {
            fn EnumConnectionPoints(&self) -> Result<IEnumConnectionPoints> {
                Err(Error::from_hresult(E_NOTIMPL))
            }
            fn FindConnectionPoint(&self, _iid: *const GUID) -> Result<IConnectionPoint> {
                Err(Error::from_hresult(HRESULT(0x80040200_u32 as i32)))
            }
        }
    };
}
no_connection_points!(Server_Impl);
no_connection_points!(Group_Impl);

#[implement(IOpcServerList)]
struct Catalog;

impl IOpcServerList_Impl for Catalog_Impl {
    unsafe fn enum_classes(
        &self,
        implemented_count: u32,
        implemented: *const GUID,
        required_count: u32,
        required: *const GUID,
        enumerator: *mut *mut c_void,
    ) -> HRESULT {
        code((|| {
            // SAFETY: The server-list ABI supplies both live category arrays and its output.
            let implemented = unsafe { input(implemented, implemented_count)? };
            // SAFETY: Required category GUIDs have the supplied count under the same contract.
            let required = unsafe { input(required, required_count)? };
            // SAFETY: Clear the enumerator output before constructing a reference.
            unsafe { output(enumerator, ptr::null_mut())? };
            let matches = implemented
                .iter()
                .chain(required)
                .all(|category| *category == DA2_CATEGORY);
            let result: IEnumGUID = Classes {
                matches,
                consumed: Mutex::new(false),
            }
            .into();
            // SAFETY: The newly constructed enumerator reference is transferred to the caller.
            unsafe { output(enumerator, result.into_raw())? };
            Ok(S_OK)
        })())
    }

    unsafe fn get_class_details(
        &self,
        class: *const GUID,
        prog_id: *mut PWSTR,
        description: *mut PWSTR,
    ) -> HRESULT {
        code((|| {
            if class.is_null() {
                return Err(Error::from_hresult(E_POINTER));
            }
            // SAFETY: These GUID/string output pointers are valid under IOPCServerList.
            unsafe {
                output(prog_id, PWSTR::null())?;
                output(description, PWSTR::null())?;
            }
            // SAFETY: The caller supplies a live CLSID.
            if unsafe { *class } != SERVER_CLASS {
                return Err(Error::from_hresult(CLASS_E_CLASSNOTAVAILABLE));
            }
            let id = task_string(PROG_ID)?;
            let name = task_string("BHTune synthetic Yokogawa loop")?;
            // SAFETY: Both allocated strings transfer ownership through the checked outputs.
            unsafe {
                output(prog_id, PWSTR(id.release()))?;
                output(description, PWSTR(name.release()))?;
            }
            Ok(S_OK)
        })())
    }

    unsafe fn class_from_prog_id(&self, prog_id: PCWSTR, class: *mut GUID) -> HRESULT {
        code((|| {
            // SAFETY: The caller supplies a live NUL-terminated ProgID input.
            if !unsafe { wide(prog_id)? }.eq_ignore_ascii_case(PROG_ID) {
                return Err(Error::from_hresult(CLASS_E_CLASSNOTAVAILABLE));
            }
            // SAFETY: The CLSID result is written to the caller's scalar output.
            unsafe { output(class, SERVER_CLASS)? };
            Ok(S_OK)
        })())
    }
}

#[implement(IEnumGUID)]
struct Classes {
    matches: bool,
    consumed: Mutex<bool>,
}

impl IEnumGUID_Impl for Classes_Impl {
    fn Next(&self, count: u32, values: *mut GUID, fetched: *mut u32) -> HRESULT {
        code((|| {
            if count == 0 || values.is_null() || (fetched.is_null() && count != 1) {
                return Err(Error::from_hresult(E_INVALIDARG));
            }
            let mut consumed = lock(&self.consumed)?;
            let present = self.matches && !*consumed;
            // SAFETY: The enumerator contract supplies count GUID slots and fetched storage.
            unsafe {
                if present {
                    output(values, SERVER_CLASS)?;
                }
                if !fetched.is_null() {
                    output(fetched, u32::from(present))?;
                }
            }

            *consumed = true;
            Ok(if present && count == 1 { S_OK } else { S_FALSE })
        })())
    }

    fn Skip(&self, count: u32) -> HRESULT {
        code((|| {
            if count == 0 {
                return Ok(S_OK);
            }
            let mut consumed = lock(&self.consumed)?;
            let present = self.matches && !*consumed;
            *consumed = true;
            Ok(if present && count == 1 { S_OK } else { S_FALSE })
        })())
    }

    fn Reset(&self) -> Result<()> {
        *lock(&self.consumed)? = false;
        Ok(())
    }

    fn Clone(&self) -> Result<IEnumGUID> {
        Ok(Classes {
            matches: self.matches,
            consumed: Mutex::new(*lock(&self.consumed)?),
        }
        .into())
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "Assertions fail the isolated native fixture tests"
)]
mod tests {
    use super::*;

    fn group(server: &IOpcServer) -> (u32, IOpcItemMgt) {
        let mut handle = 0;
        let mut rate = 0;
        let mut raw = ptr::null_mut();
        // SAFETY: All inputs/outputs are live, and the returned reference has the requested IID.
        unsafe {
            server
                .add_group(
                    windows_core::w!("smoke"),
                    BOOL(1),
                    100,
                    0,
                    ptr::null(),
                    ptr::null(),
                    1033,
                    &mut handle,
                    &mut rate,
                    &IOpcItemMgt::IID,
                    &mut raw,
                )
                .ok()
                .unwrap();
            (handle, IOpcItemMgt::from_raw(raw))
        }
    }

    fn add(items: &IOpcItemMgt, names: &[(&str, u16)]) -> Vec<(u32, HRESULT)> {
        let names = names
            .iter()
            .map(|(name, datatype)| {
                (
                    name.encode_utf16().chain([0]).collect::<Vec<_>>(),
                    *datatype,
                )
            })
            .collect::<Vec<_>>();
        let definitions = names
            .iter()
            .map(|(name, datatype)| ItemDef {
                access_path: PWSTR::null(),
                item_id: PWSTR(name.as_ptr().cast_mut()),
                active: BOOL(1),
                client_handle: 91,
                blob_size: 0,
                blob: ptr::null_mut(),
                requested_type: *datatype,
                reserved: 0,
            })
            .collect::<Vec<_>>();
        let mut results = ptr::null_mut();
        let mut errors = ptr::null_mut();
        // SAFETY: The inputs stay live, and successful outputs own exactly one element per input.
        unsafe {
            items
                .add_items(
                    definitions.len() as u32,
                    definitions.as_ptr(),
                    &mut results,
                    &mut errors,
                )
                .ok()
                .unwrap();
            let results = TaskArray {
                pointer: results,
                length: definitions.len(),
            };
            let errors = TaskArray {
                pointer: errors,
                length: definitions.len(),
            };
            (0..definitions.len())
                .map(|index| {
                    (
                        (*results.pointer.add(index)).server_handle,
                        *errors.pointer.add(index),
                    )
                })
                .collect()
        }
    }

    fn read(group: &IOpcSyncIo, handles: &[u32]) -> Vec<(Option<Value>, HRESULT, u16)> {
        let mut states = ptr::null_mut();
        let mut errors = ptr::null_mut();
        // SAFETY: Handles and outputs are live; the returned allocations have handles.len() slots.
        unsafe {
            group
                .read(
                    2,
                    handles.len() as u32,
                    handles.as_ptr(),
                    &mut states,
                    &mut errors,
                )
                .ok()
                .unwrap();
            let states = TaskArray {
                pointer: states,
                length: handles.len(),
            };
            let errors = TaskArray {
                pointer: errors,
                length: handles.len(),
            };
            (0..handles.len())
                .map(|index| {
                    let state = &*states.pointer.add(index);
                    let error = *errors.pointer.add(index);
                    (
                        if error.is_ok() {
                            Some(from_variant(&state.value).unwrap())
                        } else {
                            None
                        },
                        error,
                        state.quality,
                    )
                })
                .collect()
        }
    }

    #[test]
    fn groups_preserve_shared_constants_and_reject_bad_items_handles_and_types() {
        let shared = Arc::new(SharedPlant {
            plant: Mutex::new(Plant::new(tempfile::tempfile().unwrap())),
            started: Instant::now(),
        });
        let server: IOpcServer = Server::new(Arc::clone(&shared)).into();
        let (id, items) = group(&server);
        let added = add(
            &items,
            &[
                ("CI.Yokogawa.TC001.P", 0),
                ("unknown.PV", 0),
                ("CI.Yokogawa.TC001.MODE", VT_I4.0),
            ],
        );
        assert_eq!(added[1], (0, UNKNOWN_ITEM));
        assert_eq!(added[2], (0, BAD_TYPE));
        let io = items.cast::<IOpcSyncIo>().unwrap();
        assert_eq!(
            read(&io, &[added[0].0, 0]),
            vec![
                (Some(Value::Number(120.0)), S_OK, 0xc0),
                (None, INVALID_HANDLE, 0)
            ]
        );
        let value = to_variant(Value::Number(222.2)).unwrap();
        let mut errors = ptr::null_mut();
        // SAFETY: The single handle/VARIANT are live; the returned error array has one slot.
        unsafe {
            io.write(1, &added[0].0, &value, &mut errors).ok().unwrap();
            let errors = TaskArray {
                pointer: errors,
                length: 1,
            };
            assert_eq!(*errors.pointer, S_OK);
            server.remove_group(id, BOOL(1)).ok().unwrap();
        }
        drop(io);
        drop(items);
        drop(server);
        let server: IOpcServer = Server::new(shared).into();
        let (_, items) = group(&server);
        let added = add(&items, &[("CI.Yokogawa.TC001.P", 0)]);
        assert_eq!(
            read(&items.cast().unwrap(), &[added[0].0]),
            vec![(Some(Value::Number(222.2)), S_OK, 0xc0)]
        );
    }

    fn next_strings(enumerator: &IEnumString) -> (HRESULT, Vec<String>) {
        let mut outputs = [PWSTR::null(); 256];
        let mut fetched = 0;
        // SAFETY: The enum receives 256 writable slots and returns fetched owned COM strings.
        unsafe {
            let result = enumerator.Next(&mut outputs, Some(&mut fetched));
            let values = outputs[..fetched as usize]
                .iter()
                .map(|value| {
                    let text = wide(PCWSTR(value.0)).unwrap();
                    CoTaskMemFree(Some(value.0.cast()));
                    text
                })
                .collect();
            (result, values)
        }
    }

    #[test]
    fn native_enumerators_end_reset_and_clone_without_sharing_a_cursor() {
        let strings: IEnumString = Strings {
            names: Arc::new(vec!["PV".into(), "MV".into()]),
            cursor: Mutex::new(0),
        }
        .into();
        // SAFETY: These calls use a live fixture-owned enumerator with no external pointers.
        let clone = unsafe { strings.Clone() }.unwrap();
        assert_eq!(
            next_strings(&strings),
            (S_FALSE, vec!["PV".into(), "MV".into()])
        );
        assert_eq!(next_strings(&strings), (S_FALSE, vec![]));
        assert_eq!(
            next_strings(&clone),
            (S_FALSE, vec!["PV".into(), "MV".into()])
        );
        // SAFETY: Reset changes only this live enumerator's cursor.
        unsafe { strings.Reset() }.unwrap();
        assert_eq!(next_strings(&strings).1.len(), 2);
        let classes: IEnumGUID = Classes {
            matches: true,
            consumed: Mutex::new(false),
        }
        .into();
        let mut output = [GUID::zeroed(); 16];
        let mut fetched = 0;
        // SAFETY: All enumerator inputs/outputs are live and correctly sized.
        unsafe {
            assert_eq!(classes.Next(&mut output, Some(&mut fetched)), S_FALSE);
            assert_eq!(fetched, 1);
            assert_eq!(output[0], SERVER_CLASS);
            assert_eq!(classes.Next(&mut output, Some(&mut fetched)), S_FALSE);
            assert_eq!(fetched, 0);
            classes.Reset().unwrap();
            assert_eq!(classes.Next(&mut output[..1], Some(&mut fetched)), S_OK);
            assert_eq!(fetched, 1);
        }
    }
}
