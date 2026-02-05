.PHONY: site site-serve site-clean

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
