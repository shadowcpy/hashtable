bench:
	cargo build --profile benchmark --bins
	./benchmark.sh


build: client/* server/* shared/* Cargo.toml
	cargo build --release --bin server
	cargo build --release --bin client

clean:
	cargo clean
