// The systemd_socket module contains a lot of dead code which is only used in tests, but which I
// would like to keep up to date in case I need the module for another project.
#[allow(dead_code)]

mod systemd_socket;
mod service;
mod config;

use hyper::Request;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;

use std::os::unix::net::UnixListener as StdUnixListener;
use tokio::net::UnixListener as TokioUnixListener;
use std::io;
use std::process;
use std::path::Path;
use std::env;

fn load_config() -> config::Config {
    let args = env::args().collect::<Vec<_>>();
    if args.len() != 2 {
        eprintln!("Too {} command line arguments", if args.len() < 2 { "few" } else { "many" });
        eprintln!("Usage: {} <path/to/config.json>", args[0]);
        process::exit(1);
    }

    let config_path = Path::new(&args[1]);
    match config::Config::from_path(config_path) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("Error reading configuration: {}", e);
            process::exit(1);
        },
    }
}

fn get_listener_from_systemd() -> io::Result<TokioUnixListener> {
    let mut fds = systemd_socket::listen_fds(true).unwrap_or(vec![]);
    if fds.len() != 1 {
        eprintln!("Too {} sockets passed from systemd", if fds.len() < 1 { "few" } else { "many" });
        eprintln!("This tool only works with systemd socket activation.");
        process::exit(1);
    }
    let fd = fds.remove(0);

    // See note inside `systemd_socket::is_socket_internal` for why this is broken on Darwin.
    #[cfg(not(target_vendor = "apple"))] // See note in `is_socket_unix`.
    {
        use nix::sys::socket::SockType;

        if !systemd_socket::is_socket_unix(&fd, Some(SockType::Stream), Some(true), None)
            .unwrap_or(false)
        {
            eprintln!("The socket from systemd is not a streaming UNIX socket");
            process::exit(1);
        }
    }

    let std_listener = StdUnixListener::from(fd);
    std_listener.set_nonblocking(true)?; // Required by tokio::net::UnixListener::from_std().

    let tokio_listener = TokioUnixListener::from_std(std_listener)?;
    Ok(tokio_listener)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config = load_config();

    let listener = get_listener_from_systemd()?;

    // We start a loop to continuously accept incoming connections
    loop {
        let (stream, _) = if let Some(max_idle_time) = config.max_idle_time {
            let accept_future = listener.accept();
            let timeout_future = tokio::time::timeout(max_idle_time, accept_future);
            match timeout_future.await {
                Ok(accept_result) => accept_result,
                Err(_) => {
                    eprintln!("Timed out waiting for new connection. Exiting.");
                    process::exit(0);
                },
            }
        } else {
            listener.accept().await
        }.expect("accepting connection");

        let io = TokioIo::new(stream);
        let cfg = config.clone();

        // Spawn a tokio task to serve multiple connections concurrently.
        tokio::task::spawn(async move {
            let service = service_fn(|req: Request<hyper::body::Incoming>| {
                service::router(req, &cfg)
            });

            let conn = http1::Builder::new()
                // On OSX, disabling keep alive prevents serve_connection from
                // blocking and later returning an `Err` derived from `ENOTCONN`.
                .keep_alive(false)
                .serve_connection(io, service);

            if let Err(err) = conn.await {
                eprintln!("Error serving connection: {:?}", err);
            }
        });
    }
}
