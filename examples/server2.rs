extern crate mio;

use mio::*;
use mio::net::TcpListener;
use std::io::prelude::*;
use std::thread;
use std::sync::Arc;

fn main() {
    // Setup some tokens to allow us to identify which event is
    // for which socket.
    const SERVER: Token = Token(0);

    let addr = "0.0.0.0:2222".parse().unwrap();

    // Create a poll instance
    let poll = Arc::new(Poll::new().unwrap());

    println!("Connect");
    // Setup the listener
    let listener = Arc::new(TcpListener::bind(&addr).unwrap());

    {
        let poll = Arc::clone(&poll);
        let listener = Arc::clone(&listener);
        thread::spawn(move || {
            println!("Register");
            // Register the listener
            poll.register(&listener, SERVER, Ready::readable(),
                          PollOpt::edge()).unwrap();
        }).join().unwrap();
    }

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
