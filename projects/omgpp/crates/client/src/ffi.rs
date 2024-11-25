use crate::Client;
use omgpp_core::{
    ffi::{EndpointFFI, ToFfi},
    ConnectionState,
};
use std::{
    ffi::{c_char, c_uchar, CStr},
    net::IpAddr,
    ptr::null_mut,
    str::FromStr,
};

// FFI
type ClientOnConnectionChanged = extern "C" fn(EndpointFFI, ConnectionState);
type ClientOnMessage = extern "C" fn(EndpointFFI, i64, *const c_uchar, usize);
type ClientOnRpc = extern "C" fn(EndpointFFI, bool, i64, u64, i64, *const c_uchar, usize);

#[no_mangle]
pub unsafe extern "C" fn client_create(ip: *const c_char, port: u16) -> *mut Client {
    let c_string = CStr::from_ptr(ip).to_str();
    if c_string.is_err() {
        return null_mut();
    }

    if let Some(addres) = IpAddr::from_str(c_string.unwrap()).ok() {
        let client = Client::new(addres, port);
        Box::into_raw(Box::from(client))
    } else {
        null_mut()
    }
}

#[no_mangle]
pub unsafe extern "C" fn client_process(client: *mut Client) {
    _ = client.as_mut().unwrap().process::<128>();
}
#[no_mangle]
pub unsafe extern "C" fn client_connect(client: *mut Client) {
    client.as_mut().unwrap().connect().unwrap();
}
#[no_mangle]
pub unsafe extern "C" fn client_disconnect(client: *mut Client) {
    client.as_mut().unwrap().disconnect();
}

#[no_mangle]
pub unsafe extern "C" fn client_register_on_connection_state_change(
    client: *mut Client,
    callback: ClientOnConnectionChanged,
) {
    client
        .as_mut()
        .unwrap()
        .register_on_connection_state_changed(move |endpoint, state| {
            callback(endpoint.to_ffi(), state)
        });
}

#[no_mangle]
pub unsafe extern "C" fn client_register_on_message(
    client: *mut Client,
    callback: ClientOnMessage,
) {
    client
        .as_mut()
        .unwrap()
        .register_on_message(move |endpoint, message_id, data| {
            callback(endpoint.to_ffi(), message_id, data.as_ptr(), data.len())
        });
}
#[no_mangle]
pub unsafe extern "C" fn client_register_on_rpc(client: *mut Client, callback: ClientOnRpc) {
    client.as_mut().unwrap().register_on_rpc(
        move |endpoint, reliable, method_id, request_id, arg_type, arg_data| {
            callback(
                endpoint.to_ffi(),
                reliable,
                method_id,
                request_id,
                arg_type,
                arg_data.as_ptr(),
                arg_data.len(),
            )
        },
    );
}
#[no_mangle]
pub unsafe extern "C" fn client_send(
    client: *mut Client,
    msg_type: i64,
    data: *const c_uchar,
    offset: isize,
    size: usize,
) {
    let msg_data = core::slice::from_raw_parts(data.offset(offset), size);
    _ = client.as_mut().unwrap().send(msg_type, msg_data)
}
#[no_mangle]
pub unsafe extern "C" fn client_send_reliable(
    client: *mut Client,
    msg_type: i64,
    data: *const c_uchar,
    offset: isize,
    size: usize,
) {
    let msg_data = core::slice::from_raw_parts(data.offset(offset), size);
    _ = client.as_mut().unwrap().send_reliable(msg_type, msg_data)
}
#[no_mangle]
pub unsafe extern "C" fn client_call_rpc(
    client: *mut Client,
    reliable: bool,
    method_id: i64,
    request_id: u64,
    arg_type: i64,
    arg_data: *const c_uchar,
    arg_data_offset: isize,
    arg_data_size: usize,
) {
    let msg_data = match arg_data_size {
        0 => None,
        _ => Some(core::slice::from_raw_parts(arg_data.offset(arg_data_offset), arg_data_size)),
    };
    _ = client
        .as_ref()
        .unwrap()
        .call_rpc(reliable, method_id, request_id, arg_type, msg_data);
}

#[no_mangle]
#[allow(unreachable_patterns)]
pub unsafe extern "C" fn client_destroy(client: *mut Client) {
    match client.as_mut() {
        client_ref => {
            drop(client_ref);
        }
        _ => (),
    }
}
