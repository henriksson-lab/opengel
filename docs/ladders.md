# Commercial DNA / RNA / Protein Ladders

Reference catalog of the most commonly used molecular-weight ladders, compiled
from vendor datasheets and product pages. The machine-readable version (used by
the app's ladder dropdown and the detection engine) lives in
`src/core/ladders/ladders.json`.

Legend: **bold** = reference ("extra-thick", increased-intensity) band. Sizes in
bp (DNA), nt (RNA), or kDa (protein). Per-band ng is listed where the vendor
publishes it (mostly the DNA/RNA quantitation ladders; prestained protein
standards publish protein concentration, not per-band ng).

Provenance note: NEB DNA/ssRNA per-band masses are from NEB datasheets
(N3232, N3231, N0362). Thermo DNA per-band ng appears only as labels on each
product-page gel image and is not transcribed here (only the ~0.5 µg total load
is given in text); RNA ng is from the RiboRuler guides. Protein reference-band
colors/intensities are from vendor pages. Values not independently confirmable
to the band level are omitted rather than guessed.

---

## New England Biolabs (NEB)

### 1 kb DNA Ladder — N3232 (DNA)
https://www.neb.com/en-us/products/n3232-1-kb-dna-ladder — load 0.5 µg/lane.
10, 8, 6, 5, 4, **3**, 2, 1.5, 1, 0.5 kb.
ng/band: 42, 42, 50, 42, 33, **125**, 48, 36, 42, 42.

### 1 kb Plus DNA Ladder — N3200 (DNA)  *(formerly 2-Log DNA Ladder)*
https://www.neb.com/en-us/products/n3200-1-kb-plus-dna-ladder
10000, 8000, 6000, 5000, 4000, **3000**, 2000, 1500, 1200, **1000**, 900, 800, 700, 600, **500**, 400, 300, 200, 100 bp.

### 100 bp DNA Ladder — N3231 (DNA)
https://www.neb.com/en-us/products/n3231-100-bp-dna-ladder — load 0.5 µg/lane.
1517, 1200, **1000**, 900, 800, 700, 600, **500**, 400, 300, 200, 100 bp.
ng/band: 45, 35, **95**, 27, 24, 21, 18, **97**, 38, 29, 25, 48.

### ssRNA Ladder — N0362 (RNA)
https://www.neb.com/en-us/products/n0362-ssrna-ladder — load 1 µg (native) / 2–3 µg (denaturing).
9000, 7000, 5000, **3000**, 2000, 1000, 500 bases. (Low Range ssRNA Ladder: N0364.)

### Color Prestained Protein Standard, Broad Range — P7719 (protein)
https://www.neb.com/en-us/products/p7719-color-prestained-protein-standard-broad-range-10-250-kda
250, 180, 130, 95, **72 (orange)**, 55, 43, 34, **26 (green)**, 17, 11 kDa.

### Blue Prestained Protein Standard, Broad Range — P7718 (protein)
https://www.neb.com/en-us/products/p7718-blue-prestained-protein-standard-broad-range-11-250-kda
250, 180, 130, 95, 72, 55, 43, 34, 26, 17, 11 kDa.

### Unstained Protein Standard, Broad Range — P7717 (protein)
https://www.neb.com/en-us/products/p7717-unstained-protein-standard-broad-range-10-200-kda
200, 150, 120, 100, 85, 70, 60, 50, 40, 30, 25, 15, 10 kDa.

---

## Thermo Fisher Scientific (Thermo Scientific / Invitrogen)

DNA GeneRuler/Invitrogen: stock 0.5 µg/µL, standard load ~0.5 µg (~0.3 µg for High Range).

### GeneRuler 1 kb — SM0311 (DNA)
https://www.thermofisher.com/order/catalog/product/SM0311
10000, 8000, **6000**, 5000, 4000, 3500, **3000**, 2500, 2000, 1500, **1000**, 750, 500, 250 bp.

### GeneRuler 1 kb Plus — SM1331 (DNA)
https://www.thermofisher.com/order/catalog/product/SM1331
20000, 10000, 7000, **5000**, 4000, 3000, 2000, **1500**, 1000, 700, **500**, 400, 300, 200, 75 bp.

### GeneRuler 100 bp — SM0241 (DNA)
https://www.thermofisher.com/order/catalog/product/SM0241
1000, 900, 800, 700, 600, **500**, 400, 300, 200, 100 bp.

### GeneRuler 100 bp Plus — SM0321 (DNA)
https://www.thermofisher.com/order/catalog/product/SM0321
3000, 2000, 1500, 1200, **1000**, 900, 800, 700, 600, **500**, 400, 300, 200, 100 bp.

### GeneRuler Low Range — SM1191 (DNA)
https://www.thermofisher.com/order/catalog/product/SM1191
700, 500, 400, **300**, 200, 150, **100**, 75, 50, 25 bp.

### GeneRuler High Range — SM1351 (DNA)
https://www.thermofisher.com/order/catalog/product/SM1351
48502, 24508, 20555, 17000, 15258, 13825, 12119, 10171 bp.

### GeneRuler Ultra Low Range — SM1211 (DNA)
https://www.thermofisher.com/order/catalog/product/SM1211
300, 200, 150, 100, 75, **50**, 35, 25, 20, 15, 10 bp.

### Invitrogen 1 Kb Plus DNA Ladder — 10787018 (DNA)
https://www.thermofisher.com/order/catalog/product/10787018
15000, 10000, 8000, 7000, 6000, 5000, 4000, 3000, 2000, **1500**, 1000, 850, 650, 500, 400, 300, 200, 100 bp.

### RiboRuler High Range RNA — SM1821 (RNA)
https://www.thermofisher.com/order/catalog/product/SM1821 — 120 ng/band (~960 ng total).
6000, 4000, 3000, 2000, 1500, 1000, 500, 200 nt.

### RiboRuler Low Range RNA — SM1831 (RNA)
https://www.thermofisher.com/order/catalog/product/SM1831 — 140 ng/band (~980 ng total).
1000, 800, 600, 400, 300, 200, 100 nt.

### Millennium RNA Markers — AM7150 (RNA)
https://www.thermofisher.com/order/catalog/product/AM7150
9000, 6000, 5000, 4000, 3000, 2500, 2000, 1500, 1000, 500 nt.

### PageRuler Prestained (10–180 kDa) — 26616 (protein)
https://www.thermofisher.com/order/catalog/product/26616
180, 130, 100, **70 (orange)**, 55, 40, 35, 25, 15, **10 (green)** kDa.

### PageRuler Plus Prestained (10–250 kDa) — 26619 (protein)
https://www.thermofisher.com/order/catalog/product/26619
250, 130, 100, 70, 55, 35, 25, 15, 10 kDa.

### PageRuler Unstained (10–200 kDa) — 26614 (protein)
https://www.thermofisher.com/order/catalog/product/26614
200, 150, 120, 100, 85, 70, 60, **50**, 40, 30, 25, 20, 15, 10 kDa.

### PageRuler Unstained Broad Range (5–250 kDa) — 26630 (protein)
https://www.thermofisher.com/order/catalog/product/26630
250, 150, **100**, 70, **50**, 40, 30, **20**, 15, 10, 5 kDa.

### Spectra Multicolor Broad Range (10–260 kDa) — 26623 (protein)
https://www.thermofisher.com/order/catalog/product/26623
260, 140, 100, 70, 50, 40, 35, 25, 15, 10 kDa.

---

## Bio-Rad

### Precision Plus Protein All Blue — 1610373 (protein)
https://www.bio-rad.com/en-us/sku/1610373 — 250, 150, 100, **75**, **50**, 37, **25**, 20, 15, 10 kDa.

### Precision Plus Protein Dual Color — 1610374 (protein)
https://www.bio-rad.com/en-us/sku/1610374 — 250, 150, 100, 75 (pink), 50, 37, 25 (pink), 20, 15, 10 kDa.

### Precision Plus Protein Dual Xtra — 1610377 (protein)
https://www.bio-rad.com/en-us/sku/1610377 — 250, 150, 100, **75**, **50**, 37, **25**, 20, 15, 10, 5, **2** kDa.

### EZ Load 100 bp Molecular Ruler — 1708352 (DNA)
https://www.bio-rad.com/en-us/sku/1708352 — 1000, 900, 800, 700, 600, 500, 400, 300, **200**, 100 bp.

### EZ Load Precision Molecular Mass Ruler — 1708356 (DNA, quantitation)
https://www.bio-rad.com/en-us/sku/1708356 — total 250 ng.
1000 = 100 ng, 700 = 70 ng, 500 = 50 ng, 200 = 20 ng, 100 = 10 ng.

*(Bio-Rad does not market a common RNA size ladder.)*

---

## Promega

### 1 kb DNA Ladder — G5711 (DNA)
https://www.promega.com/products/cloning-and-dna-markers/dna-ladder-rna-ladder/1kb-dna-ladder/
10000, 8000, 6000, 5000, 4000, **3000**, 2500, 2000, 1500, **1000**, 750, 500, 250 bp.

### 100 bp DNA Ladder — G2101 (DNA)
https://www.promega.com/products/cloning-and-dna-markers/dna-ladder-rna-ladder/100bp-dna-ladder/
1500, 1000, 900, 800, 700, 600, **500**, 400, 300, 200, 100 bp.

### PCR Markers — G3161 (DNA)
https://www.promega.com/products/cloning-and-dna-markers/dna-ladder-rna-ladder/pcr-markers/
1000, 750, 500, 300, 150, 50 bp (equal intensity).

### λ DNA/HindIII Markers — G1711 (DNA)
https://www.promega.com/products/cloning-and-dna-markers/dna-ladder-rna-ladder/lambda-dna-hindiii-markers/
23130, 9416, 6557, 4361, 2322, 2027, 564, 125 bp.

### RNA Markers — G3191 (RNA)
https://www.promega.com/products/cloning-and-dna-markers/dna-ladder-rna-ladder/rna-markers/
6583, 4981, 3638, 2604, 1908, 1383, 955, 623, 281 nt.

### Broad Range Protein Molecular Weight Markers — V8491 (protein)
https://www.promega.com/products/protein-analysis/protein-molecular-weight-markers/broad-range-protein-molecular-weight-markers/
225, 150, 100, 75, **50**, 35, 25, 15, 10 kDa (50 kDa loaded at 3×).

---

## Takara Bio

### 20 bp DNA Ladder — 3409A (DNA)
https://catalog.takara-bio.co.jp/product/basic_info.php?click_flg=1&recommend_flg=1&unitid=U100003463
**500**, 400, 300, **200**, 180, 160, 140, 120, **100**, 80, 60, 40, 20 bp.

### 100 bp DNA Ladder — 3407A (DNA)
https://catalog.takara-bio.co.jp/product/basic_info.php?click_flg=1&recommend_flg=1&unitid=U100003463
1500, 1000, 900, 800, 700, 600, **500**, 400, 300, 200, 100 bp.

### 200 bp DNA Ladder — 3410A (DNA)
https://catalog.takara-bio.co.jp/product/basic_info.php?click_flg=1&recommend_flg=1&unitid=U100003463
4000, 3000, 2500, **2000**, 1800, 1600, 1400, 1200, **1000**, 800, 600, 400, 200 bp.

## GoldBio

### ReadyLadder 50 bp-25 kb DNA Ladder — D015 (DNA)
https://www.goldbio.com/products/superladder-50-bp-25-kb
25000, 10000, 8000, 6000, 5000, 4000, **3000**, 2500, 2000, 1500, **1200**, 1000, 900, 800, 700, 600, **500**, 450, 400, 350, 300, 250, **200**, 150, 100, 50 bp.

## GenScript

### Broad Multi Color Pre-Stained Protein Standard — M00624S (protein)
https://www.genscript.com/molecule/M00624S-Broad_Multi_Color_Pre_Stained_Protein_Standard.html
**270**, 175, 130, 95, 65, **50**, 35, **30**, 15, 5 kDa.

## Abcam

### Prestained Protein Ladder, Broad Range — ab116028 (protein)
https://www.abcam.com/en-us/products/standards/prestained-protein-ladder-broad-molecular-weight-10-245-kda-ab116028
245, 180, 135, 100, **75**, 63, 48, 35, **25**, 20, 17, 10 kDa.

Abcam's product page publishes the range, band count, and 75/25 kDa reference
bands in text. The exact full 12-band list is from distributor-provided Prism
Ultra/ab116028 product text and should be rechecked against an Abcam PDF/manual
if one becomes available.

## Zymo Research

### ZR small-RNA Ladder — R1090 (RNA)
https://www.govsci.com/product-detail/Zymo--Research/R1090/EA/
29, 25, 21, 17 nt.

## Lonza

### 20 bp DNA Ladder — 50330 (DNA)
https://takara.co.kr/web01/product/productList.asp?lcode=50471
500, 480, 460, 440, 420, 400, 380, 360, 340, 320, 300, 280, 260, 240, 220, 200, 180, 160, 140, 120, 100, 80, 60, 40, 20 bp.

### 50 bp-1000 bp DNA Marker — 50461 (DNA)
https://takara.co.kr/web01/product/productList.asp?lcode=50471
1000, 700, 525, 500, 400, 300, 200, 100, 50 bp.

### 100 bp Extended Range DNA Ladder — 50322 (DNA)
https://docslib.org/doc/1955291/section-iv-detection-and-sizing-of-dna-in-agarose-gels
3000 down to 100 bp in 100 bp steps.

### 1 kb-10 kb DNA Marker — 50471 (DNA)
https://takara.co.kr/web01/product/productList.asp?lcode=50471
10000, 7000, 5000, 4000, 3000, 2500, 2000, 1500, 1000 bp.

---

## Takara Bio

### 20 bp DNA Ladder — 3409A (DNA)
https://catalog.takara-bio.co.jp/product/basic_info.php?unitid=U100003463
**500**, 400, 300, **200**, 180, 160, 140, 120, **100**, 80, 60, 40, 20 bp.

### 100 bp DNA Ladder — 3407A (DNA)
https://catalog.takara-bio.co.jp/product/basic_info.php?unitid=U100003463
1500, 1000, 900, 800, 700, 600, **500**, 400, 300, 200, 100 bp.

### 200 bp DNA Ladder — 3410A (DNA)
https://catalog.takara-bio.co.jp/product/basic_info.php?unitid=U100003463
4000, 3000, 2500, **2000**, 1800, 1600, 1400, 1200, **1000**, 800, 600, 400, 200 bp.

---

## GoldBio

### ReadyLadder 50 bp–25 kb DNA Ladder — D015 (DNA)
https://www.goldbio.com/product/156/ready-to-use-50-bp-to-25-kb-dna-ladder
25000, 10000, 8000, 6000, 5000, 4000, **3000**, 2500, 2000, 1500, **1200**, 1000, 900, 800, 700, 600, **500**, 450, 400, 350, 300, 250, **200**, 150, 100, 50 bp.

---

## GenScript

### Broad Multi Color Pre-Stained Protein Standard — M00624S (protein)
https://www.genscript.com/molecule/M00624S-Broad_Multi_Color_Pre_Stained_Protein_Standard.html
**270**, 175, 130, 95, 65, **50**, 35, **30**, 15, 5 kDa.

---

## Abcam

### Prestained Protein Ladder, Broad Molecular Weight — ab116028 (protein)
https://www.abcam.com/en-us/products/standards/prestained-protein-ladder-broad-molecular-weight-10-245-kda-ab116028
245, 180, 140, 100, **75**, 60, 45, 35, **25**, 20, 15, 10 kDa.

Note: Abcam's product page publishes the 10–245 kDa range, 12-band count, and
75/25 kDa reference bands; exact apparent sizes are harmonized to the common
vendor-distributed band table for this product family.

---

## Zymo Research

### ZR small-RNA Ladder — R1090 (RNA)
https://www.zymoresearch.com/products/zr-small-rna-ladder
29, 25, 21, 17 nt.

---

## Lonza

### 20 bp DNA Ladder — 50330 (DNA)
https://www.lonza.com/products-and-services/research-solutions/electrophoresis/dna-markers-and-ladders
500, 480, 460, 440, 420, 400, 380, 360, 340, 320, 300, 280, 260, 240, 220, 200, 180, 160, 140, 120, 100, 80, 60, 40, 20 bp.

### 50 bp–1000 bp DNA Marker — 50461 (DNA)
https://www.lonza.com/products-and-services/research-solutions/electrophoresis/dna-markers-and-ladders
1000, 700, 525, 500, 400, 300, 200, 100, 50 bp.

### 100 bp Extended Range DNA Ladder — 50322 (DNA)
https://www.lonza.com/products-and-services/research-solutions/electrophoresis/dna-markers-and-ladders
3000 down to 100 bp in 100 bp increments.

### 1 kb–10 kb DNA Marker — 50471 (DNA)
https://www.lonza.com/products-and-services/research-solutions/electrophoresis/dna-markers-and-ladders
10000, 7000, 5000, 4000, 3000, 2500, 2000, 1500, 1000 bp.

---

# 2026 expansion — protein and RNA ladders

The sections below were compiled by fetching each vendor's own product page,
manual or datasheet and reading the band table there; the URL under each
heading is the document that was read. Ladders whose band list could not be
verified from a primary vendor source were left out rather than guessed —
notably Bio-Rad's prestained (non-Precision-Plus) standards and Sigma SDS7B,
whose apparent masses are calibrated per lot and printed on the vial.

Two dsRNA markers (NEB N0363, BioDynamics DM180) are filed under RNA with
"(sizes in bp)" in their name: sizing is correct, but the nmol readout uses
the 340 g/mol single-strand factor and so reads ~2x low for them.

# Protein ladders (added 61)


## Abcam

### Abcam Prestained Protein Ladder, Mid-range (10-180 kDa) — ab116027
https://www.abcam.com/en-us/products/standards/prestained-protein-ladder-mid-range-molecular-weight-10-180-kda-ab116027
180, 130, 100, **75**, 63, 48, 35, **28**, 17, 10 kDa.

### Abcam Prestained Protein Ladder, Extra Broad (5-245 kDa) — ab116029
https://www.abcam.com/en-us/products/standards/prestained-protein-ladder-extra-broad-molecular-weight-5-245-kda-ab116029
245, 180, 135, 100, **75**, 63, 48, 35, **25**, 20, 17, 11, 5 kDa.

### Abcam Prestained Protein Ladder, Mid-range Blue (10-180 kDa) — ab234617
https://www.abcam.com/en-us/products/standards/prestained-protein-ladder-mid-range-molecular-weight-10-180-kda-ab234617
180, 140, 100, **72**, 60, 45, 35, **25**, 20, 15, 10 kDa.

### Abcam Unstained Protein Ladder (10-200 kDa) — ab234618
https://www.abcam.com/en-us/products/standards/unstained-protein-ladder-10-200-kda-ab234618
200, 150, 100, **85**, 60, 50, 40, 30, **25**, 20, 15, 10 kDa.


## Bio-Rad

### Bio-Rad Precision Plus Dual Color — 1610374
https://www.bio-rad.com/webroot/web/pdf/lsr/literature/4110025.pdf
250, 150, 100, **75**, **50**, 37, **25**, 20, 15, 10 kDa.

### Bio-Rad Precision Plus Kaleidoscope — 1610375
https://www.bio-rad.com/webroot/web/pdf/lsr/literature/4110182.pdf
250, 150, 100, **75**, **50**, 37, **25**, 20, 15, 10 kDa.

### Bio-Rad Precision Plus Unstained — 1610363
https://www.bio-rad.com/webroot/web/pdf/lsr/literature/Bulletin_4110023.pdf
250, 150, 100, **75**, **50**, 37, **25**, 20, 15, 10 kDa.

### Bio-Rad Precision Plus WesternC — 1610376
https://www.bio-rad.com/webroot/web/pdf/lsr/literature/10008761B.pdf
250, 150, 100, **75**, **50**, 37, **25**, 20, 15, 10 kDa.

### Bio-Rad SDS-PAGE Standards Broad Range — 1610317
https://www.bio-rad.com/webroot/web/pdf/lsr/literature/4006035.pdf
200, 116.25, 97.4, 66.2, 45, 31, 21.5, 14.4, 6.5 kDa.

### Bio-Rad SDS-PAGE Standards Low Range — 1610304
https://www.bio-rad.com/webroot/web/pdf/lsr/literature/4006033.pdf
97.4, 66.2, 45, 31, 21.5, 14.4 kDa.

### Bio-Rad SDS-PAGE Standards High Range — 1610303
https://www.bio-rad.com/webroot/web/pdf/lsr/literature/4006034.pdf
200, 116.25, 97.4, 66.2, 45 kDa.


## Biotium

### Biotium Peacock Plus Prestained Protein Marker — 21531
https://biotium.com/product/peacock-plus-prestained-protein-marker/
245, 180, 135, 100, **75**, 63, 48, 35, **25**, 20, 17, 11 kDa.


## Cell Signaling

### Cell Signaling Prestained Protein Marker, Broad Range (11-190 kDa) — 13953
https://media.cellsignal.com/pdf/13953.pdf
190, 134, 100, 76, 57, 46, 32, 25, 22, 17, 11 kDa.

### Cell Signaling Color-coded Prestained Protein Marker, Broad Range (11-250 kDa) — 14208
https://media.cellsignal.com/pdf/14208.pdf
250, 190, 134, 100, 80, 57, 46, 32, 25, 22, 17, 11 kDa.


## Cytiva

### Cytiva Amersham ECL Rainbow Marker Full Range — RPN800E
https://cdn.cytivalifesciences.com/api/public/content/digi-15006-pdf
225, 150, 102, 76, 52, 38, 31, 24, 17, 12 kDa.

### Cytiva Amersham ECL Rainbow Marker High Range — RPN756E
https://cdn.cytivalifesciences.com/api/public/content/digi-15006-pdf
225, 76, 52, 38, 31, 24, 17, 12 kDa.

### Cytiva Amersham ECL Rainbow Marker Low Range — RPN755E
https://cdn.cytivalifesciences.com/api/public/content/digi-15006-pdf
38, 31, 24, 17, 12, 8.5, 3.5 kDa.

### Cytiva Amersham ECL Plex Fluorescent Rainbow Markers — RPN850E
https://cdn.cytivalifesciences.com/api/public/content/digi-15006-pdf
225, 150, 102, 76, 52, 38, 31, 24, 17, 12 kDa.

### Cytiva Amersham ECL DualVue Western Blotting Markers — RPN810
https://cdn.cytivalifesciences.com/api/public/content/digi-15006-pdf
150, 100, 75, 50, 35, 25, 15 kDa.

### Cytiva Amersham LMW-SDS Calibration Kit — 17-0446-01
https://pdf.dutscher.com/doc/17-0446-01/17-0446-01_MEen.pdf
97, 66, 45, 30, 20.1, 14.4 kDa.

### Cytiva Amersham HMW-SDS Calibration Kit — 17-0615-01
https://www.solarbio.com/pdf/2-DBYMY/1-ZW/17-0615-01.pdf
220, 170, 116, 76, 53 kDa.

### Cytiva Amersham HMW Native Calibration Kit (native PAGE) — 17-0445-01
https://kirschner.med.harvard.edu/files/protocols/GE_proteinelectrophoresis.pdf
669, 440, 232, 140, 67 kDa.


## GenScript

### GenScript PAGE-MASTER Protein Standard — M00516
https://www.genscript.com/gsfiles/catalog/Protein_Standards.pdf
120, 80, 60, 40, 30, 20, 10 kDa.

### GenScript PAGE-MASTER Protein Standard Plus — MM1397
https://www.genscript.com/gsfiles/catalog/Protein_Standards.pdf
120, 80, 60, 50, 40, 30, 20, 15, 10 kDa.

### GenScript WB-MASTER Protein Standard — M00521
https://www.genscript.com/gsfiles/catalog/Protein_Standards.pdf
120, 80, 60, 50, 40, 30, 20 kDa.


## GoldBio

### GoldBio BLUEstain Protein Ladder (11-245 kDa) — P007
https://www.goldbio.com/products/bluestain-protein-ladder-11-245-kda
245, 180, 140, 100, 75, 60, 45, 35, 25, 20, 15, 10 kDa.


## Jena Bioscience

### Jena Bioscience BlueEye Prestained Protein Marker (10-245 kDa) — PS-104
https://www.jenabioscience.com/images/PDF/PS-104.pdf
245, 180, 135, 100, **75**, 63, 48, 35, **25**, 20, 17, 11 kDa.

### Jena Bioscience BlueRay Prestained Protein Marker (10-180 kDa) — PS-103
https://www.jenabioscience.com/images/PDF/PS-103.0001.pdf
180, 135, 100, **75**, 63, 48, 35, **25**, 17, 11 kDa.


## LI-COR

### LI-COR Chameleon Duo Pre-stained Protein Ladder — 928-60000
https://www.licorbio.com/support/contents/reagents/chameleon-pre-stained-protein-ladder/duo.html
260, 160, 125, 90, 70, 50, 38, 30, 25, 15, 8 kDa.


## Proteintech

### Proteintech Prestained Protein Marker (10-180 kDa) — PL00001
https://www.ptglab.com/Products/Pictures/pdf/PL00001.pdf
180, 140, 100, 75, 60, 45, 35, 25, 15, 10 kDa.

### Proteintech Broad Range Prestained Protein Marker (3-245 kDa) — PL00002
https://www.ptglab.com/Products/Pictures/pdf/PL00002.pdf
245, 180, 140, 100, 75, 60, 45, 35, 25, 20, 15, 10, 3 kDa.

### Proteintech Extra Range Prestained Protein Marker (10-310 kDa) — PL00003
https://www.ptglab.com/Products/Pictures/pdf/PL00003.pdf
310, 245, 180, 140, 100, 75, 60, 45, 35, 25, 15, 10 kDa.


## SMOBIO

### SMOBIO ExcelBand All Blue Broad Range Protein Marker — PM1700
http://docs.smobio.com/document/doc/PV/PM1700.pdf
240, 180, 140, 100, **72**, 60, 45, 35, **25**, 20, 15, 10 kDa.

### SMOBIO ExcelBand 3-color Regular Range Protein Marker — PM2500
http://docs.smobio.com/document/doc/PV/PM2500.pdf
180, 140, 100, **75**, 60, 45, 35, **25**, 15, 10 kDa.


## Sigma-Aldrich

### Sigma-Aldrich SigmaMarker Wide Range (6.5-200 kDa) — S8445
https://www.sigmaaldrich.com/deepweb/assets/sigmaaldrich/product/documents/421/562/s8445bul.pdf
200, 116, 97, 66, 55, 45, 36, 29, 24, 20, 14.2, 6.5 kDa.

### Sigma-Aldrich SigmaMarker High Range (36-200 kDa) — S8320
https://www.sigmaaldrich.com/deepweb/assets/sigmaaldrich/product/documents/421/562/s8445bul.pdf
200, 116, 97, 66, 55, 45, 36 kDa.

### Sigma-Aldrich SigmaMarker Low Range (6.5-66 kDa) — M3913
https://www.sigmaaldrich.com/deepweb/assets/sigmaaldrich/product/documents/421/562/s8445bul.pdf
66, 45, 36, 29, 24, 20, 14.2, 6.5 kDa.

### Sigma-Aldrich ColorBurst Electrophoresis Marker (8-220 kDa) — C1992
https://www.sigmaaldrich.com/deepweb/assets/sigmaaldrich/product/documents/414/328/c1992bul.pdf
220, 100, 60, 45, 30, 20, 12, 8 kDa.


## Takara Bio

### Takara Protein Molecular Weight Marker (Broad) — 3452
https://www.takara.co.kr/file/manual/pdf/3452_DS.v2104Da.pdf
200, 116, 97.2, 66.409, 44.287, 29, 20.1, 14.3, 6.5 kDa.

### Takara Protein Molecular Weight Marker (Low) — 3450
https://takara.co.kr/file/manual/pdf/3450_DS.v2104Da.pdf
97.2, 66.409, 44.287, 29, 20.1, 14.3 kDa.

### Takara Protein Molecular Weight Marker (High) — 3451
https://takara.co.kr/file/manual/pdf/3451_DS.v2104Da.pdf
200, 116, 97.2, 66.409, 44.287 kDa.


## Thermo Fisher

### Invitrogen Novex Sharp Prestained (3.5-260 kDa) — LC5800
https://www.thermofisher.com/order/catalog/product/LC5800
260, 160, 110, 80, 60, 50, 40, 30, 20, 15, 10, 3.5 kDa.

### Invitrogen Novex Sharp Unstained (3.5-260 kDa) — LC5801
https://www.thermofisher.com/order/catalog/product/LC5801
260, 160, 110, 80, 60, 50, 40, 30, 20, 15, 10, 3.5 kDa.

### Invitrogen SeeBlue Plus2 Prestained (3-198 kDa) — LC5925
https://www.thermofisher.com/order/catalog/product/LC5925
198, 98, 62, 49, 38, 28, 17, 14, 6, 3 kDa.

### Invitrogen SeeBlue Prestained (3-198 kDa) — LC5625
https://www.thermofisher.com/order/catalog/product/LC5625
198, 62, 49, 38, 28, 18, 14, 6, 3 kDa.

### Invitrogen MagicMark XP Western (20-220 kDa) — LC5602
https://www.thermofisher.com/order/catalog/product/LC5602
220, 120, 100, 80, 60, 50, 40, 30, 20 kDa.

### Invitrogen BenchMark Protein Ladder (10-220 kDa) — 10747-012
https://documents.thermofisher.com/TFS-Assets/LSG/manuals/MAN0000875_10747012pps_BenchMark_Ladder_UG.pdf
220, 160, 120, 100, 90, 80, 70, 60, **50**, 40, 30, 25, **20**, 15, 10 kDa.

### Invitrogen BenchMark Prestained (6-180 kDa) — 10748-010
https://documents.thermofisher.com/TFS-Assets/LSG/manuals/MAN0000876_10748010pps_BenchMark_Prestained_Std_UG.pdf
180, 115, 82, **64**, 49, 37, 26, 19, 15, 6 kDa.

### Invitrogen BenchMark Fluorescent Protein Standard (11-155 kDa) — LC5928
https://www.thermofisher.com/order/catalog/product/LC5928
155, 98, 63, 40, 32, 21, 11 kDa.

### Invitrogen Mark12 Unstained Standard (2.5-200 kDa) — LC5677
https://www.thermofisher.com/order/catalog/product/LC5677
200, 116.3, 97.4, 66.3, 55.4, 36.5, 31, 21.5, 14.4, 6, 3.5, 2.5 kDa.

### Invitrogen HiMark Prestained HMW Standard (31-460 kDa) — LC5699
https://www.thermofisher.com/order/catalog/product/LC5699
460, 268, 238, 171, 117, 71, 55, 41, 31 kDa.

### Invitrogen HiMark Unstained HMW Standard (40-500 kDa) — LC5688
https://www.thermofisher.com/order/catalog/product/LC5688
500, 290, 240, 160, 116, 97, 66, 55, 40 kDa.

### Thermo Spectra Multicolor High Range (40-300 kDa) — 26625
https://www.thermofisher.com/order/catalog/product/26625
300, 250, 180, 130, 100, 70, 50, 40 kDa.

### Thermo Spectra Multicolor Low Range (1.7-40 kDa) — 26628
https://www.thermofisher.com/order/catalog/product/26628
40, 25, 15, 10, 4.6, 1.7 kDa.

### Thermo PageRuler Unstained High Range (60-250 kDa) — 26637
https://www.thermofisher.com/order/catalog/product/26637
250, 200, **150**, 120, 100, 85, 70, 60 kDa.

### Thermo PageRuler Unstained Low Range (3.4-100 kDa) — 26632
https://www.thermofisher.com/order/catalog/product/26632
100, 30, **25**, 20, 15, 10, 5, 3.4 kDa.

### Thermo PageRuler Prestained NIR (11-250 kDa) — 26635
https://www.thermofisher.com/order/catalog/product/26635
250, 130, 95, 70, **55**, 43, 34, 26, 15, 11 kDa.

### Pierce Unstained Protein MW Marker (14.4-116 kDa) — 26610
https://documents.thermofisher.com/TFS-Assets/LSG/manuals/MAN0011769_Unstain_Protein_Molec_Wght_Mark_UG.pdf
116, 66.2, 45, 35, 25, 18.4, 14.4 kDa.

### Pierce Prestained Protein MW Marker (20-120 kDa) — 26612
https://www.thermofisher.com/order/catalog/product/26612
120, 85, 50, 35, 25, 20 kDa.

### Thermo SuperSignal Molecular Weight Protein Ladder (20-150 kDa) — 84785
https://www.thermofisher.com/order/catalog/product/84785
150, 100, 80, 60, 50, 40, 30, 20 kDa.


## Yeasen

### Yeasen Gold Band Plus 3-color Regular Range Protein Ladder — 20350ES
https://www.yeasenbio.com/products/20350
180, 130, 100, 72, 55, 43, 33, 25, 17, 8 kDa.


# RNA ladders (added 24)


## BioDynamics

### BioDynamics DynaMarker RNA High — DM160
https://bdl-biodynamics.com/wp-content/uploads/DM160.pdf
8000, 5000, 4000, 3000, 2000, 1500, 1000, 500, 200 nt.

### BioDynamics DynaMarker RNA Low II — DM152
https://bdl-biodynamics.com/wp-content/uploads/DM152.pdf — standard load 700 ng.
500 (100 ng), 400 (100 ng), 300 (100 ng), 200 (100 ng), 100 (100 ng), 50 (100 ng), 20 (100 ng) nt.

### BioDynamics DynaMarker Small RNA II — DM192
https://bdl-biodynamics.com/wp-content/uploads/DM192.pdf
100, 50, 40, 30, 20 nt.

### BioDynamics DynaMarker dsRNA (sizes in bp) — DM180
https://bdl-biodynamics.com/wp-content/uploads/DM180.pdf
1000, 500, 400, 300, 200, 100, 50, 30, 20, 10 nt.

### BioDynamics DynaMarker Prestain Marker for RNA High — DM260
https://bdl-biodynamics.com/wp-content/uploads/DM260.pdf
8000, 4000, 2000, 1000, 500, 200 nt.

### BioDynamics DynaMarker Prestain Marker for Small RNA Plus — DM253
https://bdl-biodynamics.com/wp-content/uploads/DM253.pdf
100, 75, 50, 40, 30, 20 nt.

### BioDynamics DynaMarker DIG Labeled Blue Color Marker for Small RNA — DM270
https://bdl-biodynamics.com/wp-content/uploads/DM270.pdf
100, 75, 50, 40, 30, 20 nt.


## NEB

### NEB Low Range ssRNA Ladder — N0364
https://www.neb.com/en-us/products/n0364-low-range-ssrna-ladder
1000, 500, **300**, 150, 80, 50 nt.

### NEB dsRNA Ladder (sizes in bp) — N0363
https://www.neb.com/en-us/products/n0363-dsrna-ladder
500, 300, 150, **80**, 50, 30, 21 nt.

### NEB microRNA Marker — N2102
https://www.neb.com/en-us/products/n2102-micro-rna-marker
25, 21, 17 nt.


## Nippon Gene

### Nippon Gene RNA Ladder (0.125-6.0 kb) — 311-06261
https://www.nippongene.com/english/product/electrophoresis/rna-ladder.html
6000, 4000, 3000, 2000, 1500, 1000, 500, 250, 125 nt.


## Norgen Biotek

### Norgen 100 b RNA Ladder — 15002
https://norgenbiotek.com/sites/default/files/resources/15002-100b-RNA-Ladder.pdf
1000, 800, 600, **500**, 400, 300, 200, 100 nt.

### Norgen 1 kb RNA Ladder — 15003
https://norgenbiotek.com/sites/default/files/resources/15003-1kb-RNA-Ladder.pdf
4000, 3000, 2000, 1500, **1000**, 800, 600, 400, 200 nt.


## RefSeq

### rRNA size reference (E. coli 16S/23S) — NR_103073.1 / NR_102804.1
https://www.ncbi.nlm.nih.gov/nuccore/NR_103073.1
2904, 1542 nt.

### rRNA size reference (human 18S/28S) — NR_003287.4 / NR_003286.4
https://www.ncbi.nlm.nih.gov/nuccore/NR_003287.4
5070, 1869 nt.

### rRNA size reference (mouse 18S/28S) — NR_003279.1 / NR_003278.3
https://www.ncbi.nlm.nih.gov/nuccore/NR_003278.3
4730, 1870 nt.


## Roche

### Roche RNA Molecular Weight Marker I, DIG-labeled — 11526529910
https://www.sigmaaldrich.com/deepweb/assets/sigmaaldrich/product/documents/202/910/11373099910bul.pdf
6948, 4742, 2661, 1821, 1517, 1049, 575, 438, 310 nt.

### Roche RNA Molecular Weight Marker II, DIG-labeled — 11526537910
https://www.sigmaaldrich.com/deepweb/assets/sigmaaldrich/product/documents/202/910/11373099910bul.pdf
6948, 4742, 2661, 1821, 1517 nt.

### Roche RNA Molecular Weight Marker III, DIG-labeled — 11373099910
https://www.sigmaaldrich.com/deepweb/assets/sigmaaldrich/product/documents/202/910/11373099910bul.pdf
1517, 1049, 575, 438, 310 nt.


## Takara Bio

### Takara 0.5-10 kb ssRNA Ladder Marker — 3417A
https://www.takara.co.kr/file/manual/pdf/3417A_DS.v2311Da.pdf — standard load 950 ng.
10000 (150 ng), 8000 (100 ng), 6000 (100 ng), 4000 (100 ng), 3000 (200 ng), 2000 (100 ng), 1000 (100 ng), 500 (100 ng) nt.

### Takara 14-30 ssRNA Ladder Marker — 3416
https://catalog.takara-bio.co.jp/PDFS/3416_DS_j.pdf — standard load 268 ng.
30 (45 ng), 26 (39 ng), 22 (33 ng), 18 (109 ng), 14 (42 ng) nt.


## Thermo Fisher

### Invitrogen Century-Plus RNA Markers — AM7145
https://www.thermofisher.com/us/en/home/life-science/dna-rna-purification-analysis/nucleic-acid-gel-electrophoresis/rna-ladders/century.html
1000, 750, 500, 400, 300, 200, 100 nt.

### Invitrogen RNA 6000 Ladder — AM7152
https://www.thermofisher.com/order/catalog/product/AM7152
6000, 4000, 2000, 1000, 500, 200 nt.

### Thermo Decade Markers System — AM7778
https://documents.thermofisher.com/TFS-Assets/LSG/manuals/4386532D.pdf
150, 100, 90, 80, 70, 60, 50, 40, 30, 20, 10 nt.

