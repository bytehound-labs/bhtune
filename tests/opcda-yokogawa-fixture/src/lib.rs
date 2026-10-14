//! Test-only OPC DA 2.x COM server for the plant-free Windows CLI smoke test.

#[cfg(any(windows, test))]
mod plant;

#[cfg(windows)]
#[allow(unsafe_code, reason = "Private test-only OPC DA ABI declarations")]
mod abi;

#[cfg(windows)]
#[allow(
    unsafe_code,
    reason = "Test-only COM pointers, allocation, and DLL exports"
)]
mod com;
