# HRDEM test fixture — attribution

The two GeoTIFFs in this folder (`victoria-aoi-dtm.tif`, `victoria-aoi-dsm.tif`)
are a 140 m × 140 m crop of the Natural Resources Canada (NRCan)
**High Resolution Digital Elevation Model (HRDEM)**.

> Contains information licensed under the Open Government Licence – Canada.

- Licence: Open Government Licence – Canada, version 2.0
  <https://open.canada.ca/en/open-government-licence-canada>
- Source: NRCan HRDEM / CanElevation Series, collection `hrdem-lidar`
- STAC API: <https://datacube.services.geo.ca/stac/api>
- Item: `BC-Vancouver_Island_Sunshine_Coast_2018-1m`

See `provenance.json` for the item id, acquisition date, asset URLs,
coordinate/vertical datum, crop window and SHA-256 checksums.
Decision record: `docs/adr/018-hrdem.md`.

The magnetic (geomagnetic) reference data used elsewhere in MagGCS is **not**
NRCan and is never redistributed in this repository (see ADR-020 and the
survey plan).
