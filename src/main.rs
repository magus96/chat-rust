#![warn(rust_2018_idioms)]

use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Mutex};
use tokio_stream::StreamExt;
use tokio_util::codec::{Framed, LinesCodec};

use futures::SinkExt;
use std::collections::HashMap;
use std::env;
use std::error::Error;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>>{

    use tracing_subscriber::fmt::format::FmtSpan;
    use tracing_subscriber::EnvFilter;
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("chat-info".parse()?))
        .with_span_events(FmtSpan::FULL)
        .init();

    let state = Arc::new(Mutex::new(Shared::new()));

    let addr = env::args().nth(1).unwrap_or_else(|| "127.0.0.1:6767".to_string());

    let listener = TcpListener::bind(&addr).await?;

    tracing::info!("Server running on {}", addr);

    loop{

        let(stream, addr) = listener.accept().await?;
        let state = Arc::clone(&state);

        tokio::spawn(async move {
            tracing::debug!("accepted connection");
            if let Err(e) = process(state, stream, addr).await{
                tracing::info!("An error occured, error= {:?}", e);
            }
        });
    }

}

//Create transmitter type
type Tx = mpsc::UnboundedSender<String>;

//Create receiver type
type Rx = mpsc::UnboundedReceiver<String>;

//shared server data between peers
struct Shared{
    peers: HashMap<SocketAddr, Tx>,
}

//Connected client
struct Peer{
    lines: Framed<TcpStream, LinesCodec>,
    rx: Rx,
}


impl Shared{
    //create new 'Shared' instance(empty)
    fn new() -> Self{
        Shared { peers: HashMap::new(), }
    }

    //create broadcast function
    async fn broadcast(&mut self, sender: SocketAddr, message: &str){

        for peer in self.peers.iter_mut(){
            if *peer.0 != sender{
                let _ = peer.1.send(message.into());
            }
        }
    }
}


impl Peer{
    //Create new 'Peer' oionstance(empty)
    async fn new(state: Arc<Mutex<Shared>>, lines: Framed<TcpStream, LinesCodec>,) -> io::Result<Peer>{
        
        let addr = lines.get_ref().peer_addr()?;

        let(tx, rx) = mpsc::unbounded_channel();
        state.lock().await.peers.insert(addr,tx);
        Ok(Peer{lines, rx})
    }
}


//Process chat client
async fn process(state: Arc<Mutex<Shared>>, stream: TcpStream, addr: SocketAddr) ->Result<(), Box<dyn Error>>{

    let mut lines = Framed::new(stream, LinesCodec::new());
    lines.send("Please send your username:").await?;

    let username = match lines.next().await {
        Some(Ok(line)) => line,

        _ => {
            tracing::error!("Failed to get username from {}, Client disconencted.", addr);
            return  Ok(());
        }
    };

    let mut peer =  Peer::new(state.clone(), lines).await?;

    {
        let mut state = state.lock().await;
        let msg = format!("{} has joined the chat, Welcome {} to Ratchet", username, username);
        tracing::info!("{}", msg);
        state.broadcast(addr, &msg).await;
    }

    loop{

        tokio::select!{

            Some(msg) = peer.rx.recv() => {
                peer.lines.send(&msg).await?;
            }

            result = peer.lines.next() => match result{
                Some(Ok(msg)) => {
                    let mut state = state.lock().await;
                    let msg = format!("{}: {}", username, msg);
                    state.broadcast(addr, &msg).await;
                }

                Some(Err(e)) => {
                    tracing::error!("An error occured while processing messages for {}, error: {:?}", username, e);
                }
                None=> break,
            },
        }
    }

    {
        let mut state = state.lock().await;
        state.peers.remove(&addr);

        let msg = format!("{} has left the chat", username);
        tracing::info!("{}", msg);
        state.broadcast(addr, &msg).await;
    }
    Ok(())
}