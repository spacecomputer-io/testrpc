CONTAINER?=testflow
IMAGE?=testflow

build:
	@cargo build --bin testflow

build-release:
	@cargo build --release --bin testflow

docker-build:
	@docker build -t ${IMAGE} .

docker-rm-container:
	@docker rm -f ${CONTAINER} 2>/dev/null

docker-run: docker-rm-container
	@docker run -d --name ${CONTAINER} -e RUST_LOG=debug ${IMAGE}
