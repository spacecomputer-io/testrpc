CONTAINER?=testrpc
IMAGE?=spacecomp/testrpc:latest

build:
	@cargo build --bin testrpc

build-release:
	@cargo build --release --bin testrpc

install:
	@cargo install --locked --path .

docker-build:
	@docker build -t ${IMAGE} .

docker-rm-container:
	@docker rm -f ${CONTAINER} 2>/dev/null

docker-run: docker-rm-container
	@docker run -d --name ${CONTAINER} -e RUST_LOG=debug ${IMAGE}
