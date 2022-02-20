use {
    //boolinator::Boolinator,
    clap::{
        Arg,
        App
    },
    indicatif::{
        ProgressBar,
        ProgressStyle,
    },
    nix::ioctl_read,
    std::{
        io::SeekFrom,
        os::unix::{
            fs::FileTypeExt,
            io::AsRawFd,
        },
    },
    tokio::{
        fs::File,
        io::AsyncReadExt,
        io::AsyncWriteExt,
        io::AsyncSeekExt,
        join,
        select,
        sync::mpsc,
    },
};

// See linux/fs.h
const BLKGETSIZE64_CODE: u8 = 0x12;
const BLKGETSIZE64_SEQ: u8 = 114;

ioctl_read!(ioctl_blkgetsize64, BLKGETSIZE64_CODE, BLKGETSIZE64_SEQ, u64);

async fn get_size(f: &File) -> u64 {
    let meta = f.metadata().await.unwrap();
    let file_type = meta.file_type();

    if file_type.is_file() {
        meta.len()
    } else if file_type.is_block_device() {
        let mut size: u64 = 0;
        let size_ptr = &mut size as *mut u64;
        let std_file = f.try_clone().await.unwrap().into_std().await;
        unsafe {
            ioctl_blkgetsize64(std_file.as_raw_fd(), size_ptr).unwrap();
        }
        size
    } else {
        panic!("Only regular files, block devices and symlinks to them are supported.");
    }
}

#[tokio::main]
async fn main() {
    let matches = App::new("SSD Sync")
        .version("1.0")
        .author("Tamas Fabian <giganetom@gmail.com>")
        .about("Copy data between SSDs keeping doing as few writes as possible.")
        .arg(Arg::with_name("source")
            .short("s")
            .long("source")
            .value_name("SOURCE")
            .help("copy data from here")
            .takes_value(true)
            .required(true)
            .index(1))
        .arg(Arg::with_name("target")
            .short("t")
            .long("target")
            .value_name("TARGET")
            .help("write here")
            .takes_value(true)
            .required(true)
            .index(2))
        .get_matches();

    let source_name = matches.value_of("source").ok_or("Source name is mandatory").unwrap();
    let target_name = matches.value_of("target").ok_or("Target name is mandatory").unwrap();

    // Read both file sizes
    let mut source = File::open(source_name).await.unwrap();
    let mut target = File::open(target_name).await.unwrap();

    //let source_meta = source.metadata().await.unwrap();
    //let target_meta = target.metadata().await.unwrap();

    let source_size = get_size(&source).await;
    let target_size = get_size(&target).await;

    println!("{} -> {}", source_size, target_size);

    //(source_size == target_size).ok_or("Lengths should match").unwrap();

    let bar = ProgressBar::new(source_size);

    bar.set_style(ProgressStyle::default_bar()
        .template("{wide_bar} [{percent:>3}% {bytes_per_sec} ETA: {eta_precise}]")
        .progress_chars("##-"));

    let block_size = 1;//4096 * 4;

    let (src_tx, mut src_rx) = mpsc::channel(10);
    let (tgt_tx, mut tgt_rx) = mpsc::channel(10);
    let (wrt_tx, mut wrt_rx) = mpsc::channel::<(u64, Vec<u8>)>(10);

    // Reads source
    tokio::spawn( async move {
        let mut buf = vec![0_u8; block_size];
        loop {
            let n = match source.read(&mut buf).await {
                Err(e) => panic!("{}", e),
                Ok(n) => n
            };
            if n == 0 {
                return;
            }
            if let Err(_) = src_tx.send(buf[0..n].to_vec()).await {
                return;
            }
        }
    });

    // Reads/writes target
    tokio::spawn( async move {
        let mut buf = vec![0_u8; block_size];
        let mut readpos = 0;
        let mut seek_back = false;
        loop {
            select! {
                n_ = target.read(&mut buf) => {
                    let n = match n_ {
                        Err(e) => panic!("{}", e),
                        Ok(n) => n
                    };
                    if n == 0 {
                        return;
                    }
                    if seek_back {
                        if let Err(e) = target.seek(SeekFrom::Start(readpos)).await {
                            println!("Seek error: {}", e);
                        }
                        seek_back = false;
                    }
                    if let Err(_) = tgt_tx.send(buf[0..n].to_vec()).await {
                        return;
                    }
                    readpos += n as u64;
                }
                wr = wrt_rx.recv() => {
                    match wr {
                        Some((pos, buf)) => {
                            println!("WRITE\n");
                            // TODO: handle errors
                            if let Err(e) = target.seek(SeekFrom::Start(pos)).await {
                                println!("Seek error: {}\n", e);
                            }
                            seek_back = true;
                            if let Err(e) = target.write(&buf).await {
                                println!("Write error: {}\n", e);
                            }
                        },
                        None => {
                            // Shouldn't happen normally
                            println!("Write sender dropped, exiting.\n");
                            return;
                        }
                    }
                }
            }
        }
    });

    let mut total = 0;
    let mut diff = 0;
    let mut pos = 0;

    while let (Some(src), Some(tgt)) = join!(src_rx.recv(), tgt_rx.recv()) {
        let src_l = src.len();
        let tgt_l = tgt.len();

        if src_l == 0 || tgt_l == 0 || src_l != tgt_l {
            println!("Done.");
            break;
        }

        let n = src_l;

        bar.inc(n as u64);

        if src != tgt {
            println!("Difference at position {}\n", pos);
            if let Err(_) = wrt_tx.send((pos as u64, src)).await {
                println!("Target write receiver dropped, exiting");
                break;
            }
            diff += 1;
        }
        total += 1;
        pos += n;
    }

    bar.finish_at_current_pos();

    println!("\nTotal: {}, different: {}", total, diff);
}
