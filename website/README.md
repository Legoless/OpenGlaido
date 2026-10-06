# OpenGlaido landing page

A standalone static site. No build step, framework, external fonts, analytics, or microphone access.

From the repository root:

```sh
python3 -m http.server 4173 --bind 127.0.0.1 --directory website
```

Open <http://127.0.0.1:4173>. Deploy the contents of `website/` to any static host; the relative asset paths also work under a subdirectory. The desktop application is separate.

GitHub Pages deployment is defined in `.github/workflows/pages.yml`. It deploys on every push to `main` that touches `website/`, and can also be run manually via **Deploy website** in Actions. The workflow checks the motion controls and publishes only `index.html`, `styles.css`, `demo.js`, and `assets/`. The site is served at openglaido.com.

Pages is enabled for this public repository with **GitHub Actions** as the source; the custom domain is openglaido.com.

Run the motion-control check with `bun test scripts/landing-demo.test.ts`.

The main action links to the latest GitHub release; the repository is public and the project is licensed under MIT (see `LICENSE` at the repository root).

Aspekta is self-hosted under the included SIL Open Font License in `assets/OFL.txt`. The icon is the existing OpenGlaido app icon. The voice sculpture and signal graphics use CSS/SVG animation, with a page-wide pause control. Motion starts paused for visitors who prefer reduced motion, and the page stays static without JavaScript. The graphics are illustrative; the page never accesses the microphone.
