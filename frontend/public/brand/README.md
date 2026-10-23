# Q-Mag branding assets

- `company-dark.png`: copy of the supplied repository-root `logo_dark.png`
  (Quaternion Aerospace, transparent PNG).
- `company-light.jpg`: copy of the supplied `logo_light.jpg` for light surfaces.
- `q-mark.svg`: square Q favicon using the typeface and first letter of the
  supplied `Q-Mag-logo.svg`.

Application wordmarks are rendered by `src/components/AppLogo.tsx`, using the
source SVG's lettering with tighter bounds, a transparent background, and
theme tokens. `CompanyLogo.tsx` chooses the supplied company variant through
the resolved `data-theme` attribute. See `frontend/README.md` for placement and
verification.
