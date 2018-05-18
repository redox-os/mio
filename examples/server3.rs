extern crate mio;

use mio::*;
use mio::net::TcpListener;
use std::io::prelude::*;
use std::thread;

fn main() {
    // Setup some tokens to allow us to identify which event is
    // for which socket.
    const SERVER: Token = Token(0);

    let addr = "0.0.0.0:2222".parse().unwrap();

    // Create a poll instance
    let poll = thread::spawn(move || {
        Poll::new().unwrap()
    }).join().unwrap();

    println!("Connect");
    // Setup the listener
    let listener = TcpListener::bind(&addr).unwrap();

    println!("Register");
    // Register the listener
    poll.register(&listener, SERVER, Ready::readable() | Ready::writable(),
                  PollOpt::edge()).unwrap();

    // Create storage for events
    let mut events = Events::with_capacity(1024);

    loop {
        println!("Poll");
        poll.poll(&mut events, None).unwrap();

        for event in &events {
            println!("Event: {:?}", event);
        }

        for event in events.iter() {
            assert!(event.token() == SERVER);

            println!("Accept");
            let (mut sock, _addr) = listener.accept().unwrap();

            println!("Write");
            sock.write(b"hello world\n").unwrap();
        }
    }
}
