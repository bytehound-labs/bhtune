//! Narrow OPC DA 2.x ABI, checked against bytehound-opc-da-client 0.3.1
//! (source 3ef66a85a3c24592669ff0fc199ea85cd058c4a9).
//! Adapted from its MIT-licensed bindings; see ../LICENSE-OPC-DA-CLIENT.
//! All pointer methods require the buffers specified by the OPC DA COM contract.

#![allow(
    clippy::too_many_arguments,
    clippy::missing_safety_doc,
    reason = "Private COM ABI signatures and pointer contracts are defined by OPC DA"
)]

use std::ffi::c_void;

use windows::{Win32::Foundation::FILETIME, Win32::System::Variant::VARIANT};
use windows_core::{BOOL, GUID, HRESULT, IUnknown, IUnknown_Vtbl, PCWSTR, PWSTR, interface};

#[repr(C)]
pub struct ItemDef {
    pub access_path: PWSTR,
    pub item_id: PWSTR,
    pub active: BOOL,
    pub client_handle: u32,
    pub blob_size: u32,
    pub blob: *mut u8,
    pub requested_type: u16,
    pub reserved: u16,
}

#[repr(C)]
pub struct ItemResult {
    pub server_handle: u32,
    pub canonical_type: u16,
    pub reserved: u16,
    pub access_rights: u32,
    pub blob_size: u32,
    pub blob: *mut u8,
}

#[repr(C)]
pub struct ItemState {
    pub client_handle: u32,
    pub timestamp: FILETIME,
    pub quality: u16,
    pub reserved: u16,
    pub value: VARIANT,
}

#[repr(C)]
pub struct ServerStatus {
    pub start: FILETIME,
    pub current: FILETIME,
    pub updated: FILETIME,
    pub state: i32,
    pub group_count: u32,
    pub bandwidth: u32,
    pub major: u16,
    pub minor: u16,
    pub build: u16,
    pub reserved: u16,
    pub vendor: PWSTR,
}

#[interface("13486d50-4821-11d2-a494-3cb306c10000")]
pub unsafe trait IOpcServerList: IUnknown {
    fn enum_classes(
        &self,
        implemented_count: u32,
        implemented: *const GUID,
        required_count: u32,
        required: *const GUID,
        enumerator: *mut *mut c_void,
    ) -> HRESULT;
    fn get_class_details(
        &self,
        class: *const GUID,
        prog_id: *mut PWSTR,
        description: *mut PWSTR,
    ) -> HRESULT;
    fn class_from_prog_id(&self, prog_id: PCWSTR, class: *mut GUID) -> HRESULT;
}

#[interface("39c13a4d-011e-11d0-9675-0020afd8adb3")]
pub unsafe trait IOpcServer: IUnknown {
    pub(crate) fn add_group(
        &self,
        name: PCWSTR,
        active: BOOL,
        requested_rate: u32,
        client_handle: u32,
        time_bias: *const i32,
        deadband: *const f32,
        locale: u32,
        server_handle: *mut u32,
        revised_rate: *mut u32,
        iid: *const GUID,
        object: *mut *mut c_void,
    ) -> HRESULT;
    fn get_error_string(&self, error: HRESULT, locale: u32, text: *mut PWSTR) -> HRESULT;
    fn get_group_by_name(
        &self,
        name: PCWSTR,
        iid: *const GUID,
        object: *mut *mut c_void,
    ) -> HRESULT;
    fn get_status(&self, status: *mut *mut ServerStatus) -> HRESULT;
    pub(crate) fn remove_group(&self, handle: u32, force: BOOL) -> HRESULT;
    fn create_group_enumerator(
        &self,
        scope: i32,
        iid: *const GUID,
        object: *mut *mut c_void,
    ) -> HRESULT;
}

#[interface("f31dfde2-07b6-11d2-b2d8-0060083ba1fb")]
pub unsafe trait IOpcCommon: IUnknown {
    fn set_locale(&self, locale: u32) -> HRESULT;
    fn get_locale(&self, locale: *mut u32) -> HRESULT;
    fn query_locales(&self, count: *mut u32, locales: *mut *mut u32) -> HRESULT;
    fn get_error_string(&self, error: HRESULT, text: *mut PWSTR) -> HRESULT;
    fn set_client_name(&self, name: PCWSTR) -> HRESULT;
}

#[interface("39c13a4f-011e-11d0-9675-0020afd8adb3")]
pub unsafe trait IOpcBrowse: IUnknown {
    fn query_organization(&self, organization: *mut i32) -> HRESULT;
    fn change_position(&self, direction: i32, name: PCWSTR) -> HRESULT;
    fn browse_ids(
        &self,
        filter: i32,
        criteria: PCWSTR,
        datatype: u16,
        access_rights: u32,
        enumerator: *mut *mut c_void,
    ) -> HRESULT;
    fn get_item_id(&self, name: PCWSTR, item_id: *mut PWSTR) -> HRESULT;
    fn browse_access_paths(&self, item_id: PCWSTR, enumerator: *mut *mut c_void) -> HRESULT;
}

#[interface("39c13a72-011e-11d0-9675-0020afd8adb3")]
pub unsafe trait IOpcItemProperties: IUnknown {
    fn query_properties(
        &self,
        item_id: PCWSTR,
        count: *mut u32,
        ids: *mut *mut u32,
        descriptions: *mut *mut PWSTR,
        datatypes: *mut *mut u16,
    ) -> HRESULT;
    fn get_properties(
        &self,
        item_id: PCWSTR,
        count: u32,
        ids: *const u32,
        values: *mut *mut VARIANT,
        errors: *mut *mut HRESULT,
    ) -> HRESULT;
    fn lookup_item_ids(
        &self,
        item_id: PCWSTR,
        count: u32,
        ids: *const u32,
        item_ids: *mut *mut PWSTR,
        errors: *mut *mut HRESULT,
    ) -> HRESULT;
}

#[interface("39c13a54-011e-11d0-9675-0020afd8adb3")]
pub unsafe trait IOpcItemMgt: IUnknown {
    pub(crate) fn add_items(
        &self,
        count: u32,
        items: *const ItemDef,
        results: *mut *mut ItemResult,
        errors: *mut *mut HRESULT,
    ) -> HRESULT;
    fn validate_items(
        &self,
        count: u32,
        items: *const ItemDef,
        update_blob: BOOL,
        results: *mut *mut ItemResult,
        errors: *mut *mut HRESULT,
    ) -> HRESULT;
    fn remove_items(&self, count: u32, handles: *const u32, errors: *mut *mut HRESULT) -> HRESULT;
    fn set_active(
        &self,
        count: u32,
        handles: *const u32,
        active: BOOL,
        errors: *mut *mut HRESULT,
    ) -> HRESULT;
    fn set_client_handles(
        &self,
        count: u32,
        handles: *const u32,
        client_handles: *const u32,
        errors: *mut *mut HRESULT,
    ) -> HRESULT;
    fn set_datatypes(
        &self,
        count: u32,
        handles: *const u32,
        datatypes: *const u16,
        errors: *mut *mut HRESULT,
    ) -> HRESULT;
    fn create_enumerator(&self, iid: *const GUID, enumerator: *mut *mut c_void) -> HRESULT;
}

#[interface("39c13a50-011e-11d0-9675-0020afd8adb3")]
pub unsafe trait IOpcGroupState: IUnknown {
    fn get_state(
        &self,
        rate: *mut u32,
        active: *mut BOOL,
        name: *mut PWSTR,
        time_bias: *mut i32,
        deadband: *mut f32,
        locale: *mut u32,
        client_handle: *mut u32,
        server_handle: *mut u32,
    ) -> HRESULT;
    fn set_state(
        &self,
        requested_rate: *const u32,
        revised_rate: *mut u32,
        active: *const BOOL,
        time_bias: *const i32,
        deadband: *const f32,
        locale: *const u32,
        client_handle: *const u32,
    ) -> HRESULT;
    fn set_name(&self, name: PCWSTR) -> HRESULT;
    fn clone_group(&self, name: PCWSTR, iid: *const GUID, object: *mut *mut c_void) -> HRESULT;
}

#[interface("39c13a52-011e-11d0-9675-0020afd8adb3")]
pub unsafe trait IOpcSyncIo: IUnknown {
    pub(crate) fn read(
        &self,
        source: i32,
        count: u32,
        handles: *const u32,
        values: *mut *mut ItemState,
        errors: *mut *mut HRESULT,
    ) -> HRESULT;
    pub(crate) fn write(
        &self,
        count: u32,
        handles: *const u32,
        values: *const VARIANT,
        errors: *mut *mut HRESULT,
    ) -> HRESULT;
}

#[interface("39c13a71-011e-11d0-9675-0020afd8adb3")]
pub unsafe trait IOpcAsyncIo2: IUnknown {
    fn read(
        &self,
        count: u32,
        handles: *const u32,
        transaction: u32,
        cancel: *mut u32,
        errors: *mut *mut HRESULT,
    ) -> HRESULT;
    fn write(
        &self,
        count: u32,
        handles: *const u32,
        values: *const VARIANT,
        transaction: u32,
        cancel: *mut u32,
        errors: *mut *mut HRESULT,
    ) -> HRESULT;
    fn refresh(&self, source: i32, transaction: u32, cancel: *mut u32) -> HRESULT;
    fn cancel(&self, cancel: u32) -> HRESULT;
    fn set_enable(&self, enable: BOOL) -> HRESULT;
    fn get_enable(&self, enable: *mut BOOL) -> HRESULT;
}
