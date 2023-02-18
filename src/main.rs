use {
    clap::Parser,
    indicatif::{ProgressBar, ProgressStyle},
    nix::ioctl_read,
    std::{
        io::SeekFrom,
        os::unix::{fs::FileTypeExt, io::AsRawFd},
    },
    tokio::{
        fs::{File, OpenOptions},
        io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
        join,
        sync::mpsc,
    },
};

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    /// Source file or device
    source: String,

    /// Target file or device
    target: String,

    /// Size of blocks in bytes to read/write at once
    #[clap(short, long, default_value_t = 16 * 1024)]
    block_size: usize,
}

#[derive(PartialEq, Eq, Clone, Debug)]
struct Buf {
    length: usize,
    data: Vec<u8>,
}

impl Buf {
    fn new(size: usize) -> Self {
        Buf {
            length: 0,
            data: vec![0; size],
        }
    }

    fn as_slice(&self) -> &[u8] {
        let (ret, _) = self.data.as_slice().split_at(self.length);
        ret
    }
}

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

async fn read_blocks(
    mut file: File,
    mut buf_rx: tokio::sync::mpsc::Receiver<Buf>,
    buf_tx: tokio::sync::mpsc::Sender<Buf>,
) {
    while let Some(mut buf) = buf_rx.recv().await {
        buf.length = match file.read(&mut buf.data).await {
            Err(e) => panic!("{}", e),
            Ok(n) => n,
        };
        if let Err(_) = buf_tx.send(buf).await {
            // Nobody's listening
            return;
        }
    }
}

async fn write_blocks(
    mut f: File,
    mut buf_rx: tokio::sync::mpsc::Receiver<(u64, Buf)>,
    buf_tx: tokio::sync::mpsc::Sender<Buf>,
) {
    while let Some((pos, buf)) = buf_rx.recv().await {
        // TODO: be smart about seek. Call only when needed.
        if let Err(e) = f.seek(SeekFrom::Start(pos)).await {
            println!("Failed to seek, exiting: {}", e);
            return;
        }
        match f.write(&buf.as_slice()).await {
            Ok(written) => {
                if written != buf.length {
                    println!(
                        "Could not write {} bytes, only {}, exiting.",
                        buf.length, written
                    );
                    return;
                }
            }
            Err(e) => {
                println!("Failed to write, exiting: {}", e);
                return;
            }
        }

        // If no one needs the buffer, that's fine. We still might
        // have buffers to be written.
        let _ = buf_tx.send(buf).await;
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args = Args::parse();

    let source_name = &args.source;
    let target_name = &args.target;

    // Read both file sizes
    let source_r = File::open(source_name).await.unwrap();
    let target_r = File::open(target_name).await.unwrap();
    let target_w = OpenOptions::new()
        .write(true)
        .open(target_name)
        .await
        .unwrap();

    let source_size = get_size(&source_r).await;
    let target_size = get_size(&target_r).await;

    println!("{} -> {}", source_size, target_size);

    //(source_size == target_size).ok_or("Lengths should match").unwrap();

    let bar = ProgressBar::new(source_size);

    bar.set_style(
        ProgressStyle::default_bar()
            .template("{wide_bar} [{percent:>3}% {bytes_per_sec} ETA: {eta_precise}]")
            .expect("Template error")
            .progress_chars("##-"),
    );

    let channel_size = 8;

    // Channels for talking with the source file reader task
    let (src_fw_tx, src_fw_rx) = mpsc::channel(channel_size);
    let (src_bk_tx, mut src_bk_rx) = mpsc::channel(channel_size);

    // Channels for talking with the target file reader task
    let (tgt_r_fw_tx, tgt_r_fw_rx) = mpsc::channel(channel_size);
    let (tgt_r_bk_tx, mut tgt_r_bk_rx) = mpsc::channel(channel_size);

    // Channels for talking with the target file writer task
    let (tgt_w_fw_tx, tgt_w_fw_rx) = mpsc::channel(channel_size);

    // Source reader
    let src_r = tokio::spawn(read_blocks(source_r, src_fw_rx, src_bk_tx));

    // Target reader
    let tgt_r = tokio::spawn(read_blocks(target_r, tgt_r_fw_rx, tgt_r_bk_tx));

    // Target writer
    //
    // Connect the sorce file reader's forward channel's transmitter
    // so the written blocks immediately returned to the reader
    let tgt_w = tokio::spawn(write_blocks(target_w, tgt_w_fw_rx, src_fw_tx.clone()));

    let mut total = 0;
    let mut diff = 0;
    let mut pos = 0;

    // Send the first few buffers to the readers
    // Wait for them to be sent back
    let n_buffers = channel_size / 2;
    for _ in 0..n_buffers {
        src_fw_tx.send(Buf::new(args.block_size)).await.unwrap();
        tgt_r_fw_tx.send(Buf::new(args.block_size)).await.unwrap();
    }

    loop {
        // Get a pair of buffers from the readers
        let (bsrc, btgt) = join!(src_bk_rx.recv(), tgt_r_bk_rx.recv());
        let bsrc = bsrc.unwrap(); // TODO: handle dropped tx?
        let btgt = btgt.unwrap(); // TODO: handle dropped tx?

        // Check wether we're done
        if bsrc.length == 0 || btgt.length == 0 || bsrc.length != btgt.length {
            break;
        }

        let n = bsrc.length;

        bar.inc(n as u64);

        // Compare the arrived buffers
        // If they match:
        //   Return the buffers to the channel
        //   Wait for buffers from the readers
        //   Start from the beginning
        if bsrc == btgt {
            let _ = join!(src_fw_tx.send(bsrc), tgt_r_fw_tx.send(btgt));
            continue;
        }
        // They're different.

        // Send the one arrived from the source reader to the writer
        // Send the one arrived from the target reader back to it
        let _ = join!(tgt_w_fw_tx.send((pos, bsrc)), tgt_r_fw_tx.send(btgt));

        diff += 1;
        total += 1;
        pos += n as u64;
    }

    // Drop channels, so tasks can terminate
    drop(tgt_w_fw_tx);
    drop(src_fw_tx);
    drop(src_bk_rx);
    drop(tgt_r_fw_tx);
    drop(tgt_r_bk_rx);

    // Wait for the tasks to finish
    let _ = join!(tgt_w, src_r, tgt_r);

    bar.finish();

    println!("\nFinished. Total: {}, different: {}", total, diff);
}
