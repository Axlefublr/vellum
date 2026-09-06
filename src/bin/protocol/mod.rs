use std::borrow::Cow;
use std::io::IoSliceMut;
use std::mem::MaybeUninit;
use std::os::unix::net::UnixDatagram;

use color::DynamicColor;
use rustix::net::{
    RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, SocketAddrAny, SocketAddrUnix,
};

use crate::cli::Command;

pub const CONTROL_SOCKET: &str = "vellum.sock";

pub struct ControlSocket {
    pub socket: UnixDatagram,
}

impl ControlSocket {
    pub fn bind() -> Result<Self, String> {
        let socket = UnixDatagram::unbound().map_err(|error| error.to_string())?;
        rustix::net::sockopt::set_socket_passcred(&socket, true)
            .map_err(|error| format!("could not enable control socket credentials: {error}"))?;
        let address = SocketAddrUnix::new_abstract_name(CONTROL_SOCKET.as_bytes())
            .map_err(|error| format!("invalid control socket name: {error}"))?;
        rustix::net::bind(&socket, &address)
            .map_err(|error| format!("could not bind control socket: {error}"))?;
        socket
            .set_nonblocking(true)
            .map_err(|error| format!("could not configure control socket: {error}"))?;
        Ok(Self { socket })
    }

    pub fn recv(
        &self,
        message: &mut [u8],
    ) -> std::io::Result<Option<(usize, Option<SocketAddrAny>)>> {
        let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmCredentials(1))];
        let mut ancillary = RecvAncillaryBuffer::new(&mut space);
        let received = rustix::net::recvmsg(
            &self.socket,
            &mut [IoSliceMut::new(message)],
            &mut ancillary,
            RecvFlags::CMSG_CLOEXEC,
        )?;
        let authorized = ancillary.drain().any(|message| {
            matches!(message, RecvAncillaryMessage::ScmCredentials(credentials)
                if credentials.uid == rustix::process::geteuid())
        });
        if !authorized || received.flags.contains(rustix::net::ReturnFlags::CTRUNC) {
            return Ok(None);
        }
        Ok(Some((received.bytes, received.address)))
    }
}

impl Command {
    pub fn serialize(&self) -> Cow<'static, str> {
        match self {
            Self::Toggle => "toggle".into(),
            Self::Activate => "activate".into(),
            Self::Deactivate => "deactivate".into(),
            Self::Clear => "clear".into(),
            Self::ClearAndDeactivate => "clear_and_deactivate".into(),
            Self::IsActive => "is_active".into(),
            Self::SetColor { color } => format!("set_color={color}").into(),
            Self::IsTextEditing => "is_text_editing".into(),
            Self::Exit => "exit".into(),
        }
    }

    pub fn deserialize(message: &[u8]) -> Result<Self, &'static str> {
        match message {
            b"toggle" => Ok(Self::Toggle),
            b"activate" => Ok(Self::Activate),
            b"deactivate" => Ok(Self::Deactivate),
            b"clear" => Ok(Self::Clear),
            b"clear_and_deactivate" => Ok(Self::ClearAndDeactivate),
            b"is_active" => Ok(Self::IsActive),
            b"is_text_editing" => Ok(Self::IsTextEditing),
            b"exit" => Ok(Self::Exit),
            _ if let Some(color) = message.strip_prefix(b"set_color=") => Ok(Self::SetColor {
                color: color_from_utf8(color)?,
            }),
            _ => Err("invalid command"),
        }
    }
}

fn color_from_utf8(msg: &[u8]) -> Result<DynamicColor, &'static str> {
    std::str::from_utf8(msg)
        .map_err(|_| "expected color to be valid utf8")?
        .parse()
        .map_err(|_| "invalid color")
}
