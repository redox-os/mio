extern crate mio;

use mio::*;
use mio::net::TcpStream;
use std::io::prelude::*;

fn main() {
    // Setup some tokens to allow us to identify which event is
    // for which socket.
    const CLIENT: Token = Token(0);

    let addr = "10.0.2.2:12345".parse().unwrap();

    // Create a poll instance
    let poll = Poll::new().unwrap();

    println!("Connect");
    // Setup the client socket
    let mut sock = TcpStream::connect(&addr).unwrap();

    println!("Register");
    // Register the socket
    poll.register(&sock, CLIENT, Ready::readable(),
                  PollOpt::edge()).unwrap();

    // Create storage for events
    let mut events = Events::with_capacity(1024);

    loop {
        let mut buf = [0; 16];

        println!("Poll");
        poll.poll(&mut events, None).unwrap();

        println!("Read");
        sock.read(&mut buf).unwrap();

        for event in events.iter() {
            assert!(event.token() == CLIENT);
            println!("Write");
            sock.write(b"hello world\n").unwrap();
            println!("Buffer: {:?}", buf);
        }
    }
}
