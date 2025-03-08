//! Contains connection related API.

mod socket;
use core::fmt::Debug;

use mayheap::{String, Vec};
use serde::{Deserialize, Serialize};
pub use socket::Socket;

/// A connection.
///
/// The low-level API to send and receive messages.
#[derive(Debug)]
pub struct Connection<S: Socket> {
    socket: S,
    read_pos: usize,

    write_buffer: Vec<u8, BUFFER_SIZE>,
    method_name_buffer: String<METHOD_NAME_BUFFER_SIZE>,
    read_buffer: Vec<u8, BUFFER_SIZE>,
}

impl<S: Socket> Connection<S> {
    /// Create a new connection.
    pub fn new(socket: S) -> Self {
        Self {
            socket,
            read_pos: 0,
            write_buffer: Vec::from_slice(&[0; BUFFER_SIZE]).unwrap(),
            read_buffer: Vec::from_slice(&[0; BUFFER_SIZE]).unwrap(),
            method_name_buffer: String::new(),
        }
    }

    /// Sends a method call.
    pub async fn send_call<P>(
        &mut self,
        interface: &'static str,
        method: &'static str,
        parameters: P,
        one_way: Option<bool>,
        more: Option<bool>,
        upgrade: Option<bool>,
    ) -> crate::Result<()>
    where
        P: Serialize + Debug,
    {
        self.push_method_name(interface, method)?;

        let call = Call {
            method: &self.method_name_buffer,
            parameters,
            one_way,
            more,
            upgrade,
        };
        to_slice(&call, &mut self.write_buffer)?;

        self.socket.write(&self.write_buffer).await
    }

    /// Receives a method call reply.
    ///
    /// The generic parameters needs some explanation:
    ///
    /// * `R` is the type of the successful reply. This should be a type that can deserialize itself
    ///   from the `parameters` field of the reply.
    /// * `E` is the type of the error reply. This should be a type that can deserialize itself from
    ///   the whole reply object itself and must fail when there is no `error` field in the object.
    ///   This can be easily achieved using the `serde::Deserialize` derive:
    ///
    /// ```rust
    /// use serde::{Deserialize, Serialize};
    ///
    /// #[derive(Debug, Deserialize, Serialize)]
    /// #[serde(tag = "error", content = "parameters")]
    /// enum MyError {
    ///    // The name needs to be the fully-qualified name of the error.
    ///    #[serde(rename = "org.example.ftl.Alpha")]
    ///    Alpha { param1: u32, param2: String },
    ///    #[serde(rename = "org.example.ftl.Bravo")]
    ///    Bravo,
    ///    #[serde(rename = "org.example.ftl.Charlie")]
    ///    Charlie { param1: String },
    /// }
    /// ```
    pub async fn receive_reply<'r, Params, ReplyError>(
        &'r mut self,
    ) -> crate::Result<Result<Reply<Params>, ReplyError>>
    where
        Params: Deserialize<'r>,
        ReplyError: Deserialize<'r>,
    {
        self.read_from_socket().await?;

        // Unwrap is safe because `read_from_socket` call above ensures at least one null byte in
        // the buffer.
        let null_index = memchr::memchr(b'\0', &self.read_buffer[self.read_pos..]).unwrap();
        let buffer = &self.read_buffer[self.read_pos..null_index];
        if self.read_buffer[null_index + 1] == b'\0' {
            // This means we're reading the last message and can now reset the index.
            self.read_pos = 0;
        } else {
            self.read_pos = null_index + 1;
        }

        // First try to parse it as an error.
        // FIXME: This will mean the document will be parsed twice. We should instead try to
        // quickly check if `error` field is present and then parse to the appropriate type based on
        // that information. Perhaps a simple parser using `winnow`?
        match from_slice::<ReplyError>(buffer) {
            Ok(e) => Ok(Err(e)),
            Err(_) => from_slice::<Reply<_>>(buffer).map(Ok),
        }
    }

    // Reads at least one full message from the socket.
    async fn read_from_socket(&mut self) -> crate::Result<()> {
        if self.read_pos > 0 {
            // This means we already have at least one message in the buffer so no need to read.
            return Ok(());
        }

        let mut pos = self.read_pos;
        loop {
            let bytes_read = self.socket.read(&mut self.read_buffer[pos..]).await?;
            let total_read = pos + bytes_read;

            // This marks end of all messages. After this loop is finished, we'll have 2 consecutive
            // null bytes at the end. This is then used by the callers to determine that they've
            // read all messages and can now reset the `read_pos`.
            self.write_buffer[total_read] = b'\0';

            if self.write_buffer[total_read - 1] == b'\0' {
                // One or more full messages were read.
                break;
            }

            #[cfg(feature = "std")]
            if total_read >= self.write_buffer.len() {
                if total_read >= MAX_BUFFER_SIZE {
                    return Err(crate::Error::BufferOverflow);
                }

                self.write_buffer
                    .extend(core::iter::repeat(0).take(BUFFER_SIZE));
            }

            pos += bytes_read;
        }

        Ok(())
    }

    fn push_method_name(
        &mut self,
        interface: &'static str,
        method: &'static str,
    ) -> crate::Result<()> {
        self.method_name_buffer
            .push_str(interface)
            .map_err(|_| crate::Error::BufferOverflow)?;
        self.method_name_buffer
            .push('.')
            .map_err(|_| crate::Error::BufferOverflow)?;
        self.method_name_buffer
            .push_str(method)
            .map_err(|_| crate::Error::BufferOverflow)?;

        Ok(())
    }
}

/// A successful method call reply.
#[derive(Debug, Serialize, Deserialize)]
pub struct Reply<Params> {
    parameters: Params,
    continues: Option<bool>,
}

impl<Params> Reply<Params> {
    /// The parameters of the reply.
    pub fn parameters(&self) -> &Params {
        &self.parameters
    }

    /// If there are more replies to come.
    pub fn continues(&self) -> Option<bool> {
        self.continues
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Call<'c, P> {
    method: &'c str,
    parameters: P,
    one_way: Option<bool>,
    more: Option<bool>,
    upgrade: Option<bool>,
}

// TODO: Cargo features to customize buffer sizes.
const BUFFER_SIZE: usize = 1024;
#[cfg(feature = "std")]
const MAX_BUFFER_SIZE: usize = 1024 * 1024; // Don't allow buffers over 1MB.
const METHOD_NAME_BUFFER_SIZE: usize = 256;

fn from_slice<'a, T>(buffer: &'a [u8]) -> crate::Result<T>
where
    T: Deserialize<'a>,
{
    #[cfg(feature = "std")]
    {
        serde_json::from_slice::<T>(buffer).map_err(Into::into)
    }

    #[cfg(not(feature = "std"))]
    {
        serde_json_core::from_slice::<T>(buffer)
            .map_err(Into::into)
            .map(|(e, _)| e)
    }
}

fn to_slice<T>(value: &T, buf: &mut [u8]) -> crate::Result<()>
where
    T: Serialize + ?Sized,
{
    #[cfg(feature = "std")]
    {
        serde_json::to_writer(buf, value).map_err(Into::into)
    }

    #[cfg(not(feature = "std"))]
    {
        serde_json_core::to_slice(value, buf)
            .map_err(Into::into)
            .map(|_| ())
    }
}
