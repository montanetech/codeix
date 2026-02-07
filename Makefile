.PHONY: build site site-serve site-clean bench bench-speed bench-quality bench-value

# Build
build:
	cargo build --release

# Site
site: site-prep
	cd site && zola build

site-serve: site-prep
	cd site && zola serve

site-prep:
	mkdir -p site/static/schemas site/static/spec site/content/spec
	cp spec/*.schema.json site/static/schemas/
	cp spec/*.schema.json site/static/spec/
	cp spec/codeindex.md site/content/spec/_index.md

site-clean:
	rm -rf site/public site/static/schemas site/static/spec site/content/spec/_index.md

# Benchmarks
bench:
	@echo "Usage: make bench-speed | bench-quality | bench-value"
	@echo "  bench-speed    - Quantitative indexing speed benchmark"
	@echo "  bench-quality  - A/B: prod codeix vs dev codeix"
	@echo "  bench-value    - A/B: codeix vs raw Claude"

bench-speed: build
	python -m scripts.bench index-speed

bench-quality: build
	python -m scripts.bench search-quality

bench-value: build
	python -m scripts.bench search-value
