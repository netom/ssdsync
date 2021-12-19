use {
    clap::{
        Arg,
        App,
        SubCommand,
    },
    indicatif::{
        ProgressBar,
        ProgressStyle,
    }
};

struct Args {

}

fn main() {
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

    let source_name = matches.value_of("source");
    let target_name = matches.value_of("target");

    let steps = 2000000;

    let bar = ProgressBar::new(steps);

    bar.set_style(ProgressStyle::default_bar()
        .template("{wide_bar} [{percent:>3}% ETA: {eta_precise}, {elapsed_precise} / {duration_precise}]")
        .progress_chars("##-"));

    // Read both file sizes

    // Sizes must match

    // Determine the number of blocks to read

    // Start loop. In the loop

    // Read two N-blocks in parallel

    // Determine the blocks to write

    // Write differing blocks

    // Repeat

    for _ in 0..steps {
        bar.inc(1);
    }

}
