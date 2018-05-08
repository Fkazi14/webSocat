#![allow(unused,dead_code)]

extern crate tokio_uds;

use std;
use tokio_core::reactor::{Handle};
use futures;
use futures::future::Future;
use futures::unsync::oneshot::{Receiver,Sender,channel};
use futures::stream::Stream;
use tokio_io::{AsyncRead,AsyncWrite};
use std::io::{Read,Write};
use std::io::Result as IoResult;
use std::net::SocketAddr;

use std::rc::Rc;
use std::cell::RefCell;

use ::std::fs::{File};
use ::std::path::{Path,PathBuf};

use self::tokio_uds::{UnixStream,UnixListener,UnixDatagram};


use super::{Peer, wouldblock, BoxedNewPeerFuture, BoxedNewPeerStream, peer_err_s, box_up_err};
use super::{once,multi,Specifier,ProgramState,PeerConstructor,Options};



#[derive(Debug,Clone)]
pub struct UnixConnect(pub PathBuf);
impl Specifier for UnixConnect {
    fn construct(&self, h:&Handle, _: &mut ProgramState, _opts: &Options) -> PeerConstructor {
        once(unix_connect_peer(h, &self.0))
    }
    specifier_boilerplate!(noglobalstate singleconnect no_subspec typ=Other);
}

#[derive(Debug,Clone)]
pub struct UnixListen(pub PathBuf);
impl Specifier for UnixListen {
    fn construct(&self, h:&Handle, _: &mut ProgramState, _opts: &Options) -> PeerConstructor {
        multi(unix_listen_peer(h, &self.0))
    }
    specifier_boilerplate!(noglobalstate multiconnect no_subspec typ=Other);
}

#[derive(Debug,Clone)]
pub struct UnixDgram(pub PathBuf, pub PathBuf);
impl Specifier for UnixDgram {
    fn construct(&self, h:&Handle, _: &mut ProgramState, opts: &Options) -> PeerConstructor {
        once(dgram_peer(h, &self.0, &self.1, opts))
    }
    specifier_boilerplate!(noglobalstate singleconnect no_subspec typ=Other);
}

fn to_abstract(x: &str) -> PathBuf {
    format!("\x00{}", x).into()
}

#[derive(Debug,Clone)]
pub struct AbstractConnect(pub String);
impl Specifier for AbstractConnect {
    fn construct(&self, h:&Handle, _: &mut ProgramState, _opts: &Options) -> PeerConstructor {
        once(unix_connect_peer(h, &to_abstract(&self.0)))
    }
    specifier_boilerplate!(noglobalstate singleconnect no_subspec typ=Other);
}

#[derive(Debug,Clone)]
pub struct AbstractListen(pub String);
impl Specifier for AbstractListen {
    fn construct(&self, h:&Handle, _: &mut ProgramState, _opts: &Options) -> PeerConstructor {
        multi(unix_listen_peer(h, &to_abstract(&self.0)))
    }
    specifier_boilerplate!(noglobalstate multiconnect no_subspec typ=Other);
}

#[derive(Debug,Clone)]
pub struct AbstractDgram(pub String, pub String);
impl Specifier for AbstractDgram {
    fn construct(&self, h:&Handle, _: &mut ProgramState, opts: &Options) -> PeerConstructor {
        #[cfg(not(feature="workaround1"))]
        {once(dgram_peer(h, &to_abstract(&self.0), &to_abstract(&self.1), opts))}
        #[cfg(feature="workaround1")]
        {once(dgram_peer_workaround(h, &to_abstract(&self.0), &to_abstract(&self.1), opts))}
    }
    specifier_boilerplate!(noglobalstate singleconnect no_subspec typ=Other);
}


// based on https://github.com/tokio-rs/tokio-core/blob/master/examples/proxy.rs
#[derive(Clone)]
struct MyUnixStream(Rc<UnixStream>, bool);

impl Read for MyUnixStream {
    fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
        (&*self.0).read(buf)
    }
}

impl Write for MyUnixStream {
    fn write(&mut self, buf: &[u8]) -> IoResult<usize> {
        (&*self.0).write(buf)
    }

    fn flush(&mut self) -> IoResult<()> {
        Ok(())
    }
}

impl AsyncRead for MyUnixStream {}

impl AsyncWrite for MyUnixStream {
    fn shutdown(&mut self) -> futures::Poll<(), std::io::Error> {
        try!(self.0.shutdown(std::net::Shutdown::Write));
        Ok(().into())
    }
}

impl Drop for MyUnixStream {
    fn drop(&mut self) {
        let i_am_read_part = self.1;
        if i_am_read_part {
            let _ = self.0.shutdown(std::net::Shutdown::Read);
        }
    }
}

pub fn unix_connect_peer(handle: &Handle, addr: &Path) -> BoxedNewPeerFuture {
    Box::new(
      futures::future::result(
        UnixStream::connect(&addr, handle).map(|x| {
            info!("Connected to a unix socket");
            let x = Rc::new(x);
            Peer::new(MyUnixStream(x.clone(), true), MyUnixStream(x.clone(), false))
        }).map_err(box_up_err)
      )
    ) as BoxedNewPeerFuture
}

pub fn unix_listen_peer(handle: &Handle, addr: &Path) -> BoxedNewPeerStream {
    let bound = match UnixListener::bind(&addr, handle) {
        Ok(x) => x,
        Err(e) => return peer_err_s(e),
    };
    // TODO: chmod / unlink
    Box::new(
        bound
        .incoming()
        .map(|(x, _addr)| {
            info!("Incoming unix socket connection");
            let x = Rc::new(x);
            Peer::new(MyUnixStream(x.clone(), true), MyUnixStream(x.clone(), false))
        })
        .map_err(|e|box_up_err(e))
    ) as BoxedNewPeerStream
}


struct DgramPeer {
    s : UnixDatagram,
    oneshot_mode: bool,
}

#[derive(Clone)]
struct DgramPeerHandle(Rc<RefCell<DgramPeer>>);

pub fn dgram_peer(handle: &Handle, bindaddr: &Path, connectaddr: &Path, opts: &Options) -> BoxedNewPeerFuture {
    Box::new(
        futures::future::result(
            UnixDatagram::bind(bindaddr, handle).and_then(|x| {
                x.connect(connectaddr)?;
            
                let h1 = DgramPeerHandle(Rc::new(RefCell::new(
                DgramPeer {
                    s: x,
                    oneshot_mode: opts.udp_oneshot_mode,
                })));
                let h2 = h1.clone();
                Ok(Peer::new(h1, h2))
            }).map_err(box_up_err)
        )
    ) as BoxedNewPeerFuture
}

#[cfg(feature="workaround1")]
extern crate libc;
#[cfg(feature="workaround1")]
pub fn dgram_peer_workaround(handle: &Handle, bindaddr: &Path, connectaddr: &Path, opts: &Options) -> BoxedNewPeerFuture {
    info!("Workaround method for getting abstract datagram socket");
    fn getfd(bindaddr: &Path, connectaddr: &Path) -> Option<i32> {
        use self::libc::{
            socket,AF_UNIX,SOCK_DGRAM,bind,connect,
            sockaddr,sockaddr_un,sa_family_t,socklen_t,
            c_char,
        };
        use ::std::mem::{size_of,transmute};
        use ::std::os::unix::ffi::OsStrExt;
        unsafe {
            let s = socket(AF_UNIX,SOCK_DGRAM,0);
            if s == -1 {
                return None;
            }
            {
                let mut sa = sockaddr_un { sun_family: AF_UNIX as sa_family_t, sun_path: [0;108] };
                let bp : &[c_char] = transmute(bindaddr.as_os_str().as_bytes());
                let l = 108.min(bp.len());
                sa.sun_path[..l].copy_from_slice(&bp[..l]);
                let sa_len = l + size_of::<sa_family_t>();
                let ret = bind(s, transmute(&sa), sa_len as socklen_t);
                if ret == -1 {
                    return None
                }
            }
            {
                let mut sa = sockaddr_un { sun_family: AF_UNIX as sa_family_t, sun_path: [0;108] };
                let bp : &[c_char] = transmute(connectaddr.as_os_str().as_bytes());
                let l = 108.min(bp.len());
                sa.sun_path[..l].copy_from_slice(&bp[..l]);
                let sa_len = l + size_of::<sa_family_t>();
                let ret = connect(s, transmute(&sa), sa_len as socklen_t);
                if ret == -1 {
                    return None
                }
            }
            Some(s)
        }
    }
    fn getpeer(handle:&Handle,bindaddr: &Path, connectaddr: &Path, opts:&Options) -> Result<Peer,Box<::std::error::Error>> {
        if let Some(fd) = getfd(bindaddr, connectaddr) {
            let s : ::std::os::unix::net::UnixDatagram = unsafe {
                ::std::os::unix::io::FromRawFd::from_raw_fd(fd)
            };
            let ss = UnixDatagram::from_datagram(s, handle)?;
            let h1 = DgramPeerHandle(Rc::new(RefCell::new(
                DgramPeer {
                    s: ss,
                    oneshot_mode: opts.udp_oneshot_mode,
                })));
            let h2 = h1.clone();
            Ok(Peer::new(h1, h2))
        } else {
            Err("Failed to get, bind or connect socket")?
        }
    }
    Box::new(
        futures::future::result({
            getpeer(handle, bindaddr, connectaddr, opts)
        })
    ) as BoxedNewPeerFuture
}


impl Read for DgramPeerHandle {
    fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
        let mut p = self.0.borrow_mut();
        p.s.recv(buf)
    }
}

impl Write for DgramPeerHandle {
    fn write(&mut self, buf: &[u8]) -> IoResult<usize> {
        let mut p = self.0.borrow_mut();
        p.s.send(buf)
    }

    fn flush(&mut self) -> IoResult<()> {
        Ok(())
    }
}

impl AsyncRead for DgramPeerHandle {}

impl AsyncWrite for DgramPeerHandle {
    fn shutdown(&mut self) -> futures::Poll<(), std::io::Error> {
        Ok(().into())
    }
}
