build: client/* server/* shared/* Cargo.toml
	cargo build --release --bins

bench:
	cargo build --profile benchmark --bins
	./benchmark.sh

perf:
	cargo build --profile benchmark --bins
	./perf.sh

clean:
	cargo clean
