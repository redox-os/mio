use event::Event;
use std::collections::{BTreeMap, BTreeSet};
use std::mem;
use std::os::unix::io::{RawFd, AsRawFd};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering, ATOMIC_USIZE_INIT};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use sys::redox;
use syscall::{self, O_RDWR, O_CLOEXEC, EVENT_READ, EVENT_WRITE, close, fevent, read, open};
use {io, Ready, PollOpt, Token};

/// Each Selector has a globally unique(ish) ID associated with it. This ID
/// gets tracked by `TcpStream`, `TcpListener`, etc... when they are first
/// registered with the `Selector`. If a type that is previously associatd with
/// a `Selector` attempts to register itself with a different `Selector`, the
/// operation will return with an error. This matches windows behavior.
static NEXT_ID: AtomicUsize = ATOMIC_USIZE_INIT;

const RELOAD: Token = Token(::std::usize::MAX - 123);

enum SelectorEvent {
    Register {
        fd: RawFd,
        flags: usize,
        token: Token,
        ret: mpsc::SyncSender<io::Result<()>>
    },
    Deregister {
        fd: RawFd,
        ret: mpsc::SyncSender<io::Result<()>>
    },
    Wait {
        ret: mpsc::SyncSender<io::Result<Events>>
    },
    Stop
}

pub struct Selector {
    id: usize,
    thread: Option<thread::JoinHandle<()>>,
    events: Mutex<mpsc::Sender<SelectorEvent>>,
    reload: redox::Awakener
}

impl Selector {
    pub fn new() -> io::Result<Selector> {
        let reload = redox::Awakener::new()?;
        let reload_fd = reload.reader().as_raw_fd();

        // offset by 1 to avoid choosing 0 as the id of a selector
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed) + 1;

        let (tx, rx) = mpsc::channel();

        // redox needs everything to be on one thread
        // because threads don't share event queue
        let thread = thread::spawn(move || {
            let efd = open("event:", O_RDWR | O_CLOEXEC).map_err(super::from_syscall_error).unwrap();

            let mut tokens = BTreeMap::new();

            fevent(reload_fd, EVENT_READ).unwrap();

            loop {
                match rx.recv().unwrap() {
                    SelectorEvent::Stop => {
                        println!("cleaning up");
                        close(efd).unwrap();
                        break;
                    },
                    SelectorEvent::Register { fd, flags, token, ret } => {
                        println!("Register: {} to token {:?} with flags {}", fd, token, flags);

                        ret.send(fevent(fd, flags)
                                .map_err(super::from_syscall_error)
                                .map(|_| {
                                    tokens.entry(fd)
                                        .or_insert(BTreeSet::new()).insert(token);
                                }))
                            .unwrap();
                    },
                    SelectorEvent::Deregister { fd, ret } => {
                        println!("Deregister: {}", fd);

                        ret.send(fevent(fd, 0)
                                .map_err(super::from_syscall_error)
                                .map(|_| { tokens.remove(&fd); }))
                            .unwrap();
                    },
                    SelectorEvent::Wait { ret } => {
                        //for &fd in tokens.keys() {
                        //    let mut buf = [0; 32];
                        //    println!("read {}: {:?}", fd, read(fd, &mut buf));
                        //}

                        use std::slice;

                        let mut dst = [syscall::Event::default(); 128];

                        println!("selecting between {} things...", tokens.len());
                        let result = read(efd, unsafe {
                            slice::from_raw_parts_mut(
                                dst.as_mut_ptr() as *mut u8,
                                dst.len() * mem::size_of::<syscall::Event>()
                            )
                        });
                        let cnt = match result {
                            Ok(n) => n / mem::size_of::<syscall::Event>(),
                            Err(err) => {
                                ret.send(Err(super::from_syscall_error(err))).unwrap();
                                break;
                            }
                        };
                        println!("select done!!111");

                        let mut evts = Events::with_capacity(1);

                        for event in &dst[..cnt] {
                            let mut kind = Ready::empty();

                            if event.flags & EVENT_READ == EVENT_READ {
                                kind = kind | Ready::readable();
                            }
                            if event.flags & EVENT_WRITE == EVENT_WRITE {
                                kind = kind | Ready::writable();
                            }

                            if let Some(tokens) = tokens.get(&event.id) {
                                for token in tokens.iter() {
                                    println!("event: {{ kind: {:?}, token: {:?} }}", kind, token);
                                    evts.push_event(Event::new(kind, token.clone()));
                                }
                            }
                        }

                        ret.send(Ok(evts)).unwrap();
                    }
                }
            }
        });

        Ok(Selector {
            id: id,
            thread: Some(thread),
            events: Mutex::new(tx),
            reload: reload
        })
    }

    pub fn id(&self) -> usize {
        self.id
    }

    /// Wait for events from the OS
    pub fn select(&self, evts: &mut Events, awakener: Token, _timeout: Option<Duration>) -> io::Result<bool> {
        let (tx, rx) = mpsc::sync_channel(1);

        println!("pls select?");
        self.events.lock().unwrap().send(SelectorEvent::Wait {
            ret: tx
        }).unwrap();

        rx.recv().unwrap().map(|return_evts| {
            *evts = return_evts;
            let mut found_awakener = false;

            evts.events.retain(|e| {
                let token = e.token();
                if token == awakener {
                    found_awakener = true;
                    return false;
                } else if token == RELOAD {
                    self.reload.cleanup();
                    return false;
                }

                true
            });

            found_awakener
        })
    }

    /// Register event interests for the given IO handle with the OS
    pub fn register(&self, fd: RawFd, token: Token, interests: Ready, opts: PollOpt) -> io::Result<()> {
        let flags = ioevent_to_fevent(interests, opts);
        //fevent(fd, flags).map_err(super::from_syscall_error)?;

        let (tx, rx) = mpsc::sync_channel(1);

        println!("Scheduling register: {}", fd);
        self.events.lock().unwrap().send(SelectorEvent::Register {
            fd: fd,
            flags: flags,
            token: token,
            ret: tx
        }).unwrap();

        self.reload.wakeup()?;
        rx.recv().unwrap()
    }

    /// Register event interests for the given IO handle with the OS
    pub fn reregister(&self, fd: RawFd, token: Token, interests: Ready, opts: PollOpt) -> io::Result<()> {
        self.register(fd, token, interests, opts)
    }

    /// Deregister event interests for the given IO handle with the OS
    pub fn deregister(&self, fd: RawFd) -> io::Result<()> {
        let (tx, rx) = mpsc::sync_channel(1);

        println!("Scheduling deregister: {}", fd);
        self.events.lock().unwrap().send(SelectorEvent::Deregister {
            fd: fd,
            ret: tx
        }).unwrap();

        self.reload.wakeup()?;
        rx.recv().unwrap()
    }
}

fn ioevent_to_fevent(interest: Ready, _opts: PollOpt) -> usize {
    let mut flags = 0;

    if interest.is_readable() {
        flags |= EVENT_READ;
    }
    if interest.is_writable() {
        flags |= EVENT_WRITE;
    }

    flags
}

impl Drop for Selector {
    fn drop(&mut self) {
        self.events.lock().unwrap().send(SelectorEvent::Stop).unwrap();
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

pub struct Events {
    events: Vec<Event>,
}

impl Events {
    pub fn with_capacity(u: usize) -> Events {
        Events { events: Vec::with_capacity(u) }
    }
    pub fn len(&self) -> usize {
        self.events.len()
    }
    pub fn capacity(&self) -> usize {
        self.events.capacity()
    }
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
    pub fn get(&self, idx: usize) -> Option<Event> {
        self.events.get(idx).map(|e| e.clone())
    }
    pub fn push_event(&mut self, event: Event) {
        self.events.push(event);
    }
    pub fn clear(&mut self) {
        self.events.clear();
    }
}
