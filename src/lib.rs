use std::env;
use std::io;
use std::io::BufReader;
use std::io::prelude::*;
use std::process::Command;
use std::process::Stdio;

pub enum HTTP {
    _400,
    _500,
}

enum Header {
    Key,
    WS,
    Value
}

macro_rules! warn {
    ($fmt:expr) => ((writeln!(io::stderr(), $fmt)).unwrap());
    ($fmt:expr, $($arg:tt)*) => ((writeln!(io::stderr(), $fmt, $($arg)*)).unwrap());
}

macro_rules! debug {
    ($fmt:expr) => (
        match option_env!("CGID_DEBUG") {
            None => (),
            Some(_) => warn!($fmt),
        }
    );
    ($fmt:expr, $($arg:tt)*) => (
        match option_env!("CGID_DEBUG") {
            None => (),
            Some(_) => warn!($fmt, $($arg)*),
        }
    );
}

fn early_exit(line: &str) -> ! {
    print!("HTTP/1.0 {}\r\n", line);
    std::process::exit(1);
}

/// Parses header into (key, value) tuple
///
/// # Examples
///
/// ```
/// use cgid;
///
/// let result = cgid::parse_header(&"key: value\r".to_string());
///
/// let (k, v) = result.unwrap();
/// assert_eq!(k, "KEY");
/// assert_eq!(v, "value");
/// ```
///
/// It returns an Err if the header is malformed:
///
/// ```
/// use cgid;
///
/// let result = cgid::parse_header(&"key=value".to_string());
///
/// assert!(result.is_err());
/// ```
///
pub fn parse_header(line: &String) -> Result<(String, String), ()> {
    let mut key: Vec<char> = Vec::new();
    let mut value: Vec<char> = Vec::new();
    let mut state = Header::Key;
    let mut valid = false;
    for c in line.chars() {
        match state {
            Header::Key => {
                if c == ':' {
                    valid = true;
                    state = Header::WS;
                } else {
                    key.push(c)
                }
            }
            Header::WS => {
                if c != ' ' {
                    value.push(c);
                    state = Header::Value;
                }
            }
            Header::Value => {
                if c == '\r' {
                    break;
                }
                value.push(c)
            }
        }
    }
    if !valid {
        return Err(());
    };
    return Ok((
        key.iter().cloned().collect::<String>()
            .to_uppercase()
            .replace("-", "_"),
        value.iter().cloned().collect::<String>()
    ));
}

/// Sets an environment variable parsed from `line`
///
/// `line` must contain a `key` `value` pair separated by a `: ` (colon + optional trailing spaces).
/// "key: value"
///
/// The environment variable name will be `key`: prefixed by 'HTTP_',
/// converted to upper_case, and `-` replaced with `_`.
///
/// The value will have leading spaces removed but is otherwise unmodified.
///
/// # Examples
///
/// ```
/// use cgid;
/// use std::env;
///
/// let mut content_length: usize = 0;
/// let result = cgid::set_header("key: value".to_string(), &mut content_length);
///
/// assert!(result.is_ok());
/// assert_eq!(env::var("HTTP_KEY").unwrap(), "value");
/// ```
///
pub fn set_header(line: String, content_length: &mut usize) -> Result<(), HTTP> {
    let (key, value) = match parse_header(&line) {
        Ok((k, v)) => (k, v),
        Err(_) => return Err(HTTP::_400),
    };
    let mut env_key = "HTTP_".to_owned();
    env_key.push_str(&key);

    if env_key == "HTTP_CONTENT_TYPE" {
        env_key = String::from("CONTENT_TYPE");
    } else if env_key == "HTTP_CONTENT_LENGTH" {
        env_key = String::from("CONTENT_LENGTH");
        match value.parse::<usize>() {
            Ok(n) => { *content_length = n },
            Err(_) => return Err(HTTP::_400),
        }
    }
    debug!("HEADER: {}={}", env_key, value);
    env::set_var(env_key, value);
    Ok(())
}

enum Req {
    Method,
    PathInfo,
    QueryString,
    Protocol
}

fn set_request(line: &String) {
    let mut method: Vec<char> = Vec::new();
    let mut path_info: Vec<char> = Vec::new();
    let mut query_string: Vec<char> = Vec::new();
    let mut server_protocol: Vec<char> = Vec::new();
    let mut state = Req::Method;

    for c in line.chars() {
        match state {
            Req::Method => {
                if c == ' ' {
                    debug!("METHOD: {}", method.iter().cloned().collect::<String>());
                    state = Req::PathInfo;
                } else {
                    method.push(c);
                }
            }
            Req::PathInfo => {
                if c == '?' {
                    state = Req::QueryString;
                    debug!("PATH_INFO: {}", path_info.iter().cloned().collect::<String>());
                } else if c == ' ' {
                    state = Req::Protocol;
                    debug!("PATH_INFO: {}", path_info.iter().cloned().collect::<String>());
                } else {
                    path_info.push(c);
                }
            }
            Req::QueryString => {
                if c == ' ' {
                    state = Req::Protocol;
                    debug!("QUERY_STRING: {}", query_string.iter().cloned().collect::<String>());
                } else {
                    query_string.push(c);
                }
            }
            Req::Protocol => {
                if c == '\n' || c == '\r' {
                    debug!("SERVER_PROTOCOL: {}", server_protocol.iter().cloned().collect::<String>());
                    break;
                }
                server_protocol.push(c);
            }
        }
    }

    env::set_var("REQUEST_METHOD", method.iter().cloned().collect::<String>());
    env::set_var("SCRIPT_NAME", "");
    env::set_var("PATH_INFO", path_info.iter().cloned().collect::<String>());
    env::set_var("QUERY_STRING", query_string.iter().cloned().collect::<String>());
    env::set_var("SERVER_PROTOCOL", server_protocol.iter().cloned().collect::<String>());
}

pub fn main() {
    env::set_var("GATEWAY_INTERFACE", "CGI/1.1");
    env::set_var("SERVER_SOFTWARE", "cgid/0.1.0");
    env::set_var("SERVER_NAME", env::var("TCPLOCALIP").unwrap_or_else(|e| {
        warn!("Couldn't get TCPLOCALIP (not running under UCSPI?): {}", e);
        warn!("Defaulting to 127.0.0.1");
        String::from("127.0.0.1")
    }));
    env::set_var("SERVER_PORT", env::var("TCPLOCALPORT").unwrap_or_else(|e| {
        warn!("Couldn't get TCPLOCALPORT (not running under UCSPI?): {}", e);
        warn!("Defaulting to 80");
        String::from("80")
    }));

    let stdin = io::stdin();

    let mut content_length: usize = 0;

    debug!("\n\n\n");
    let mut req = String::new();
    stdin.lock().read_line(&mut req).unwrap_or_else(|e| {
        warn!("WTF how can there not be a line: {}", e);
        early_exit("500 Internal Server Error");
    });

    set_request(&req);
    warn!("REQUEST: {}", req);

    debug!("Request header set!\n");

    for line in stdin.lock().lines() {
        let val = line.unwrap_or_else(|e| {
            warn!("WTF how can there not be a line: {}", e);
            early_exit("500 Internal Server Error");
        });
        if val == "" {
            break;
        }
        match set_header(val, &mut content_length) {
            Ok(_) => (),
            Err(HTTP::_400) => early_exit("400 Invalid Header"),
            Err(_) => early_exit("500 Internal Server Error"),
        }
    }

    debug!("All headers set!\n");

    let args: Vec<_> = env::args().collect();

    let mut child: Command = Command::new(args[1].clone());
    for i in 2..args.len() {
        child.arg(args[i].clone());
    }
    child.stdin(Stdio::piped())
        .stdout(Stdio::piped());
    let f = child.spawn().unwrap_or_else(|e| {
        warn!("Failed to execute child: {}", e);
        early_exit("500 Internal Server Error");
    });

    let mut c_stdin = f.stdin.unwrap_or_else(|| {
        warn!("Failed to get child's STDIN");
        early_exit("500 Internal Server Error");
    });
    debug!("Writing STDIN to child's STDIN...");
    copy_exact(&mut io::stdin(), &mut c_stdin, content_length).unwrap_or_else(|e| {
        warn!("Failed to copy child's STDIN: {}", e);
        early_exit("500 Internal Server Error");
    });
    debug!("Written.");

    // Note that this is where Content-Length would be recorded and passed, but
    // because it would incur more memory overhead and it would be a hassle, Content-Length is not
    // supported.  Maybe I'll add support optionally
    let c_stdout = f.stdout.unwrap_or_else(|| {
        warn!("Failed to get child's STDOUT");
        early_exit("500 Internal Server Error");
    });
    let mut reader = BufReader::new(c_stdout);
    debug!("Writing child's STDOUT to STDOUT...");
    loop {
        let mut val = String::new();
        reader.read_line(&mut val).unwrap_or_else(|e| {
            warn!("WTF how can there not be a line: {}", e);
            early_exit("500 Internal Server Error");
        });
        let (key, value) = match parse_header(&val) {
            Ok((k, v)) => (k, v),
            Err(_) => {
                warn!("Invalid header: {}", val);
                early_exit("500 Internal Server Error");
            }
        };
        if key == String::from("STATUS") {
            print!("HTTP/1.0 {}\r\n", value);
            // flush buffered headers
            break;
        } else {
            // Buffer skipped headers
        }
    }
    io::copy(&mut reader, &mut io::stdout()).unwrap_or_else(|e| {
        // XXX: note that if this happens who knows what got written to STDOUT; the 500 may end up
        // in the middle of a file or something crazy like that, but what can you do?
        warn!("Failed to copy child's STDOUT: {}", e);
        early_exit("500 Internal Server Error");
    });
    debug!("Written.");
}

fn copy_exact<R: Read, W: Write>(mut reader: R, mut writer: W,
        length: usize) -> Result<(), std::io::Error> {
    const BUFFER_SIZE: usize = 64 * 1024;
    let mut buffer: Vec<u8> = vec![0; BUFFER_SIZE];

    let mut buffer_left = length;
    while buffer_left > BUFFER_SIZE {
        try!(reader.read_exact(&mut buffer));
        try!(writer.write_all(&buffer));
        buffer_left -= BUFFER_SIZE;
    }

    try!(reader.read_exact(&mut buffer[..buffer_left]));
    try!(writer.write_all(&buffer[..buffer_left]));
    Ok(())
}
