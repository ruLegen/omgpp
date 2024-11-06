use bimap::BiHashMap;
use md5;
use std::{collections::HashMap, fmt::Debug, marker::PhantomData, net::IpAddr, sync::LazyLock};

use gns::{
    GnsConnection, GnsConnectionEvent, GnsConnectionInfo, GnsGlobal, GnsNetworkMessage, GnsSocket,
    GnsUtils, IsCreated, IsServer, ToReceive, ToSend,
};
use gns_sys::{
    k_nSteamNetworkingSend_Reliable, k_nSteamNetworkingSend_Unreliable, EResult,
    ESteamNetworkingConnectionState,
};
use uuid::Uuid;

type OnConnectRequestCallback = Box<dyn Fn(&Uuid) -> bool + Send + 'static>;
type OnConnectionChangedCallback = Box<dyn Fn(&Uuid, ConnectionState) + Send + 'static>;
type OnMessageCallback = Box<dyn Fn(&Uuid, i32, Vec<u8>) + Send + 'static>;

type ServerResult<T> = Result<T, String>; // TODO replace error with enum

struct GnsWrapper {
    global: GnsGlobal,
    utils: GnsUtils,
}
unsafe impl Send for GnsWrapper {}
unsafe impl Sync for GnsWrapper {}

static GNS: LazyLock<ServerResult<GnsWrapper>> = LazyLock::new(|| {
    Ok(GnsWrapper {
        global: GnsGlobal::get()?,
        utils: GnsUtils::new().ok_or("Error occurred when creating GnsUtils")?,
    })
});

#[allow(dead_code)]
#[derive(Debug)]
pub enum ConnectionState {
    Disconnected = 0,
    Disconnecting = 1,
    Connecting = 2,
    Connected = 3,
}
struct ServerCallbacks {
    on_connect_requested_callback: OnConnectRequestCallback,
    on_connection_changed_callback: Option<OnConnectionChangedCallback>,
    on_message_callback: Option<OnMessageCallback>,
}
pub struct Server<'a> {
    ip: IpAddr,
    port: u16,
    active_connetions: BiHashMap<Uuid, GnsConnection>,
    socket: GnsSocket<'static, 'static, IsServer>,
    callbacks: ServerCallbacks,
    phantom: PhantomData<&'a bool>,
}

impl<'a> Server<'a> {
    pub fn new(ip: IpAddr, port: u16) -> ServerResult<Server<'a>> {
        let gns = GNS.as_ref()?;
        let gns_socket = GnsSocket::<IsCreated>::new(&gns.global, &gns.utils).unwrap();
        let address_to_bind = match ip {
            IpAddr::V4(v4) => v4.to_ipv6_mapped(),
            IpAddr::V6(v6) => v6,
        };
        let server_socket = gns_socket
            .listen(address_to_bind, port)
            .or(ServerResult::Err("Cannot create server socket".to_string()))?;

        Ok(Server {
            ip,
            port,
            socket: server_socket,
            active_connetions: BiHashMap::new(),
            callbacks: ServerCallbacks {
                on_connect_requested_callback: Box::new(|_id| true),
                on_connection_changed_callback: None,
                on_message_callback: None,
            },
            phantom: Default::default(),
        })
    }
    /// Make 1 server cycle.
    /// Generic paramter N specfies maximum number of events and messages to process per a call
    pub fn process<const N: usize>(&mut self) -> ServerResult<()> {
        let socket = &self.socket;
        socket.poll_callbacks();
        let mut socket_op_result = ServerResult::Ok(());
        let _processed_event_count = socket.poll_event::<N>(|event| {
            socket_op_result = Server::process_connection_events(
                event,
                &self.socket,
                &self.callbacks,
                &mut self.active_connetions,
            )
        });
        let _processed_msg_count = socket.poll_messages::<N>(|msg| {
            socket_op_result =
                Server::process_messages(msg, &self.active_connetions, &self.callbacks)
        });

        socket_op_result
    }

    pub fn send(&self, player: &Uuid, data: &[u8]) -> ServerResult<()> {
        let connection = self
            .active_connetions
            .get_by_left(player)
            .ok_or_else(|| "There is not such player to send")?;
        self.send_with_flags(connection.clone(), k_nSteamNetworkingSend_Unreliable, data)
    }
    pub fn send_reliable(&self, player: &Uuid, data: &[u8]) -> ServerResult<()> {
        let connection = self
            .active_connetions
            .get_by_left(player)
            .ok_or_else(|| "There is not such player to send")?;
        self.send_with_flags(connection.clone(), k_nSteamNetworkingSend_Reliable, data)
    }

    pub fn broadcast(&self, data: &[u8]) -> ServerResult<()> {
        self.broadcast_with_flags(k_nSteamNetworkingSend_Unreliable, data)
    }
    pub fn broadcast_reliable(&self, data: &[u8]) -> ServerResult<()> {
        self.broadcast_with_flags(k_nSteamNetworkingSend_Reliable, data)
    }

    pub fn register_on_connect_requested(
        &mut self,
        callback: impl Fn(&Uuid) -> bool + 'static + Send,
    ) {
        self.callbacks.on_connect_requested_callback = Box::from(callback);
    }
    pub fn register_on_connection_state_changed(
        &mut self,
        callback: impl Fn(&Uuid, ConnectionState) + 'static + Send,
    ) {
        self.callbacks.on_connection_changed_callback = Some(Box::from(callback));
    }
    pub fn register_on_message(&mut self, callback: impl Fn(&Uuid, i32, Vec<u8>) + 'static + Send) {
        self.callbacks.on_message_callback = Some(Box::from(callback));
    }

    fn process_connection_events(
        event: GnsConnectionEvent,
        socket: &GnsSocket<IsServer>,
        callbacks: &ServerCallbacks,
        active_connetions: &mut BiHashMap<Uuid, GnsConnection>,
    ) -> ServerResult<()> {
        let player_uuid = Server::generate_uuid(&event.info());
        match (event.old_state(), event.info().state()) {
            // player tries to connect
            (
                ESteamNetworkingConnectionState::k_ESteamNetworkingConnectionState_None,
                ESteamNetworkingConnectionState::k_ESteamNetworkingConnectionState_Connecting,
            ) => {
                if let Some(cb) = &callbacks.on_connection_changed_callback{
                    cb(&player_uuid, ConnectionState::Connecting);
                }
                let should_accept = (callbacks.on_connect_requested_callback)(&player_uuid);
                if should_accept {
                    socket.accept(event.connection()).or_else(|_err| {
                        ServerResult::Err("Cannot accept the connection".to_string())
                    })?;
                } else {
                    // watch all possible reasons in ESteamNetConnectionEnd at steamworks_sdk_160\sdk\public\steam\steamnetworkingtypes.h (SteamworksSDK)
                    socket.close_connection(
                        event.connection(),
                        0,      // k_ESteamNetConnectionEnd_Invalid 
                        "You are not allowed to connect",
                        false,
                    );
                }
            }
            // player disconnected gracefully (? or may be not)
            (
                ESteamNetworkingConnectionState::k_ESteamNetworkingConnectionState_Connecting
                | ESteamNetworkingConnectionState::k_ESteamNetworkingConnectionState_Connected,
                 ESteamNetworkingConnectionState::k_ESteamNetworkingConnectionState_ClosedByPeer
                |ESteamNetworkingConnectionState::k_ESteamNetworkingConnectionState_ProblemDetectedLocally,
            ) => {
                if active_connetions.contains_left(&player_uuid){
                    active_connetions.remove_by_left(&player_uuid);
                }
                if let Some(cb) = &callbacks.on_connection_changed_callback {
                    cb(&player_uuid, ConnectionState::Disconnected);
                }
            }
            // player connected
            (
                ESteamNetworkingConnectionState::k_ESteamNetworkingConnectionState_Connecting,
                ESteamNetworkingConnectionState::k_ESteamNetworkingConnectionState_Connected,
            ) => {
                active_connetions.insert(player_uuid.clone(),event.connection());

                if let Some(cb) = &callbacks.on_connection_changed_callback {
                    cb(&player_uuid, ConnectionState::Connected);
                }
            }

            (_, _) => (),
        }
        Ok(())
    }

    fn process_messages(
        event: &GnsNetworkMessage<ToReceive>,
        tracked_connections: &BiHashMap<Uuid, GnsConnection>,
        callbacks: &ServerCallbacks,
    ) -> ServerResult<()> {
        let data = event.payload();
        let connection = event.connection();
        let sender = tracked_connections
            .get_by_right(&connection)
            .ok_or_else(|| "Unknown connection".to_string())?;
        // cb stands for callback
        if let Some(cb) = &callbacks.on_message_callback {
            cb(sender, 0, Vec::from(data));
        }
        Ok(())
    }

    fn broadcast_with_flags(&self, flags: i32, data: &[u8]) -> ServerResult<()> {
        let active_connections = &self.active_connetions;
        let connections = active_connections
            .into_iter()
            .map(|item| item.1.clone())
            .map(|connection| {
                self.socket
                    .utils()
                    .allocate_message(connection, flags, data)
            })
            .collect::<Vec<GnsNetworkMessage<ToSend>>>();
        if connections.len() > 0 {
            let res = self.socket.send_messages(connections);
            // TODO handle the send result
        }
        Ok(())
    }
    fn send_with_flags(
        &self,
        connection: GnsConnection,
        flags: i32,
        data: &[u8],
    ) -> ServerResult<()> {
        let res = self.socket.send_messages(vec![self
            .socket
            .utils()
            .allocate_message(connection, flags, data)]);

        if res.get(0).unwrap().is_right() {
            return ServerResult::Err("Some error occured when sending the message".to_string());
        }
        Ok(())
    }

    fn generate_uuid(info: &GnsConnectionInfo) -> Uuid {
        let hash_str = format!(
            "{}:{}",
            info.remote_address().to_string(),
            info.remote_port().to_string()
        );
        let hash_digest = md5::compute(hash_str);

        Uuid::from_bytes(hash_digest.0)
    }
}

impl<'a> Debug for Server<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Server")
            .field("ip", &self.ip)
            .field("port", &self.port)
            .field("active_connetions", &self.active_connetions)
            .finish()
    }
}
