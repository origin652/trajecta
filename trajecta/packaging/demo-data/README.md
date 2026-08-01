# Trajecta CFSR demonstration data

This archive contains four unchanged CFSR `pgbl00` GRIB2 frames used by the
Trajecta quickstart. They cover 00, 06, 12, and 18 UTC on 1 January 2009.

## Source

The files came from the NOAA National Centers for Environmental Information
(NCEI) CFSR six-hourly low-resolution archive:

https://www.ncei.noaa.gov/data/climate-forecast-system/access/reanalysis/6-hourly-low-resolution/2009/200901/20090101/

The official CFSR metadata record identifies GRBLOW as the low-resolution data
product within the CFSR collection:

https://www.ncei.noaa.gov/access/metadata/landing-page/bin/iso?id=gov.noaa.ncdc:C00765

## Use conditions

The NCEI record asks users to cite the dataset. Its Use Constraints and Access
Constraints disclaim warranties and liability, including liability associated
with distribution. The record does not list a restriction on electronic
redistribution. NOAA describes its data as publicly available and its open-data
program as providing full and open access:

https://www.noaa.gov/information-technology/open-data-dissemination

Trajecta therefore republishes these four source files unchanged, with their
original names, byte sizes, SHA-256 identities, source URLs, and this notice.
No NOAA endorsement is implied. The MIT License covers the Trajecta packaging
code and metadata; it does not replace the source-data conditions.

Please cite:

> Saha, S., S. Moorthi, H. Pan, X. Wu, J. Wang, and coauthors, 2010: The NCEP
> Climate Forecast System Reanalysis. *Bulletin of the American Meteorological
> Society*, 91, 1015–1057. https://doi.org/10.1175/2010BAMS3001.1

NCEI also supplies this acknowledgement:

> The CFSR data was developed by NOAA's National Centers for Environmental
> Prediction (NCEP). The data for this study are from NOAA's National
> Operational Model Archive and Distribution System (NOMADS), which is
> maintained at NOAA's National Centers for Environmental Information (NCEI).

Copy the four files from `data/` to `examples/domain-fill-cfsr/data/`. Keep
`MANIFEST.json` with the archive as its provenance record.
