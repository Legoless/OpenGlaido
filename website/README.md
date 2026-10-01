# OpenGlaido landing page

A standalone static site. No build step, framework, external fonts, analytics, or microphone access.

From the repository root:

```sh
python3 -m http.server 4173 --bind 127.0.0.1 --directory website
```

Open <http://127.0.0.1:4173>. Deploy the contents of `website/` to any static host; the relative asset paths also work under a subdirectory. The desktop application is separate.

GitHub Pages deployment is defined in `.github/workflows/pages.yml`. After enabling Pages with **GitHub Actions** as the source in the repository settings, run **Deploy website** from Actions on `main`. The workflow checks the motion controls and publishes only `index.html`, `styles.css`, `demo.js`, and `assets/`.

The current account plan does not support Pages for this private repository. Keep deployment manual until the account has a supported plan or the repository is intentionally made public. Hosting from a private repository requires a GitHub plan that supports Pages for private repositories; the published landing page is public.

Run the motion-control check with `bun test scripts/landing-demo.test.ts`.

The main action links to the GitHub repository. The repository is currently private and has no published releases, so the page makes repository access explicit in the setup FAQ. When public releases are available, update that answer and replace the main GitHub action with a release link if desired. Confirm the project license before adding a license claim.

Aspekta is self-hosted under the included SIL Open Font License in `assets/OFL.txt`. The icon is the existing OpenGlaido app icon. The voice sculpture and signal graphics use CSS/SVG animation, with a page-wide pause control. Motion starts paused for visitors who prefer reduced motion, and the page stays static without JavaScript. The graphics are illustrative; the page never accesses the microphone.
