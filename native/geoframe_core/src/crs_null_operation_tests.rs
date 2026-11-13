//! The operation `to_crs` picks for datums where PROJ's first operation is a null one (v0.1.2 C2).
//!
//! GDA94 / GDA2020 (EPSG "to WGS 84 (1)" is a no-op), RD/83, and datums such as ED50, Minna and
//! Saba whose several operations PROJ chooses from by area of use and accuracy. The numbers come
//! from pyproj 3.8.0 / PROJ 9.8.1 (`always_xy`, no grids installed):
//!
//! ```python
//! t = pyproj.Transformer.from_crs("EPSG:4326", f"EPSG:{code}", always_xy=True)
//! x, y = t.transform(lon, lat)
//! ```

use crate::transform_xy;

const CASES: &[(&str, f64, f64, f64, f64)] = &[
    ("EPSG:4283", 133.0, -25.0, 133.0, -25.0),
    ("EPSG:4283", 151.2, -33.9, 151.2, -33.9),
    ("EPSG:4283", 115.86, -31.95, 115.86, -31.95),
    (
        "EPSG:28355",
        151.2,
        -33.9,
        888447.1826766436,
        6240982.385726567,
    ),
    (
        "EPSG:28355",
        149.1,
        -35.3,
        690940.766492111,
        6091664.69515107,
    ),
    ("EPSG:7844", 133.0, -25.0, 133.0, -25.0),
    ("EPSG:7844", 115.86, -31.95, 115.86, -31.95),
    (
        "EPSG:7855",
        151.2,
        -33.9,
        888447.1826766436,
        6240982.385726567,
    ),
    (
        "EPSG:7855",
        149.1,
        -35.3,
        690940.766492111,
        6091664.69515107,
    ),
    (
        "EPSG:4230",
        -3.7,
        40.4,
        -3.6986948715816697,
        40.401181063243286,
    ),
    (
        "EPSG:4230",
        10.75,
        59.91,
        10.751378027873864,
        59.91045126855421,
    ),
    (
        "EPSG:4230",
        18.95,
        69.65,
        18.951586440186663,
        69.649958661662,
    ),
    ("EPSG:4230", 25.7, 66.5, 25.70107543028068, 66.5000462015333),
    ("EPSG:4230", 2.3, 48.8, 2.301273391445271, 48.80091350890631),
    (
        "EPSG:4230",
        32.8,
        39.9,
        32.80033786471777,
        39.90097874574326,
    ),
    (
        "EPSG:4230",
        10.0,
        52.0,
        10.001156586799151,
        52.00073651548477,
    ),
    (
        "EPSG:4230",
        12.5,
        41.9,
        12.500926063155882,
        41.90099008126812,
    ),
    (
        "EPSG:4230",
        25.0,
        60.2,
        25.00088941690149,
        60.20031694890364,
    ),
    (
        "EPSG:4230",
        23.7,
        38.0,
        23.700623507315626,
        38.00099671780099,
    ),
    (
        "EPSG:4230",
        -9.14,
        38.72,
        -9.138600732186957,
        38.72118760221552,
    ),
    (
        "EPSG:23030",
        -3.7,
        40.4,
        440707.1824014402,
        4472597.434580409,
    ),
    (
        "EPSG:23030",
        -1.0,
        38.0,
        675715.1990773486,
        4207911.1512708375,
    ),
    (
        "EPSG:23030",
        2.17,
        41.39,
        932414.9844196113,
        4595178.747335581,
    ),
    ("EPSG:4263", 7.5, 9.0, 7.500729320908731, 8.999737919061108),
    ("EPSG:4263", 3.4, 6.5, 3.400789981338041, 6.499512969565332),
    (
        "EPSG:4263",
        12.0,
        10.0,
        12.000655226356802,
        9.999819673273974,
    ),
    ("EPSG:4263", 40.0, 10.0, 40.00011041127623, 9.99978681272968),
    ("EPSG:26332", 7.5, 9.0, 335187.9342746474, 995070.1001221294),
    (
        "EPSG:26332",
        10.0,
        6.0,
        610752.6827618128,
        663184.8303809577,
    ),
    (
        "EPSG:10636",
        -63.23,
        17.63,
        -63.23062821171481,
        17.62920113416057,
    ),
    (
        "EPSG:10736",
        -62.98,
        17.49,
        -62.98062294331343,
        17.489253628806086,
    ),
    (
        "EPSG:4745",
        13.4,
        52.5,
        13.401749832411273,
        52.50140809017192,
    ),
    (
        "EPSG:4745",
        11.0,
        51.0,
        11.001350534641313,
        51.00124586212827,
    ),
    (
        "EPSG:4745",
        0.0,
        0.0,
        8.517629044829427e-05,
        -0.0039653965109366605,
    ),
    (
        "EPSG:4223",
        10.2,
        36.8,
        10.199411997881228,
        36.79851706935897,
    ),
    (
        "EPSG:4168",
        -0.2,
        5.6,
        -0.2002924903982743,
        5.597202361314354,
    ),
    (
        "EPSG:4208",
        -38.5,
        -12.9,
        -38.50129936973828,
        -12.898393574523556,
    ),
];

#[test]
fn null_and_helmert_operations_are_chosen_like_proj() {
    let mut failures = Vec::new();
    for (code, lon, lat, x, y) in CASES {
        // geographic results are degrees (5e-7 is about 5 cm), projected ones metres
        let tolerance = if x.abs() <= 360.0 && y.abs() <= 90.0 {
            5e-7
        } else {
            0.05
        };
        match transform_xy("EPSG:4326", code, *lon, *lat) {
            Err(e) => failures.push(format!("{code} at {lon} {lat}: {e}")),
            Ok((gx, gy)) if (gx - x).abs() > tolerance || (gy - y).abs() > tolerance => {
                failures.push(format!(
                    "{code} at {lon} {lat}: {gx} {gy} vs pyproj {x} {y}"
                ));
            }
            Ok(_) => {}
        }
    }
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

// -- ESRI text (WKT1_ESRI, no identifier): the same operation as by code (v0.1.2 C2 part 2) -------
//
// 26 projected CRSs that were refused before C1 answered from their `.prj` text with a datum error
// of up to 316 m. Those on a Greenwich datum PROJ does not shift (GDA2020, GSK-2011) or shifts by
// an operation of the table (RD/83, PD/83) answer like pyproj; the numbers are those of
//
// ```python
// wkt = re.sub(r',AUTHORITY\["[A-Z]+","[A-Za-z0-9_]+"\]\]$', "]", crs.to_wkt("WKT1_ESRI"))
// x, y = pyproj.Transformer.from_crs(4326, wkt, always_xy=True).transform(lon, lat)
// back = pyproj.Transformer.from_crs(wkt, 4326, always_xy=True).transform(x, y)
// ```
/// CRS label, ESRI text, and points `(lon, lat, x, y, back_lon, back_lat)`.
type EsriCase = (
    &'static str,
    &'static str,
    [(f64, f64, f64, f64, f64, f64); 2],
);

const ESRI_CASES: &[EsriCase] = &[
    (
        "EPSG:3398",
        r#"PROJCS["RD/83_GK_Zone_4",GEOGCS["GCS_RD/83",DATUM["D_Rauenberg_1983",SPHEROID["Bessel_1841",6377397.155,299.1528128]],PRIMEM["Greenwich",0.0],UNIT["Degree",0.0174532925199433]],PROJECTION["Gauss_Kruger"],PARAMETER["False_Easting",4500000.0],PARAMETER["False_Northing",0.0],PARAMETER["Central_Meridian",12.0],PARAMETER["Scale_Factor",1.0],PARAMETER["Latitude_Of_Origin",0.0],UNIT["Meter",1.0]]"#,
        [
            (
                12.7,
                50.93,
                4549317.894372173,
                5644090.099169454,
                12.700000006325677,
                50.930000002543615,
            ),
            (
                13.1,
                51.23,
                4576939.672032707,
                5677807.178007957,
                13.100000006664278,
                51.23000000259797,
            ),
        ],
    ),
    (
        "EPSG:9473",
        r#"PROJCS["GDA2020_Australian_Albers",GEOGCS["GDA2020",DATUM["GDA2020",SPHEROID["GRS_1980",6378137.0,298.257222101]],PRIMEM["Greenwich",0.0],UNIT["Degree",0.0174532925199433]],PROJECTION["Albers"],PARAMETER["False_Easting",0.0],PARAMETER["False_Northing",0.0],PARAMETER["Central_Meridian",132.0],PARAMETER["Standard_Parallel_1",-18.0],PARAMETER["Standard_Parallel_2",-36.0],PARAMETER["Latitude_Of_Origin",0.0],UNIT["Meter",1.0]]"#,
        [
            (
                133.27,
                -26.78,
                124755.68441287364,
                -2902763.1991995326,
                133.27,
                -26.78000000000001,
            ),
            (
                133.67,
                -26.48,
                164486.68013701515,
                -2869569.5155386245,
                133.67,
                -26.479999999999986,
            ),
        ],
    ),
    (
        "EPSG:20904",
        r#"PROJCS["GSK-2011_Gauss-Kruger_zone_4",GEOGCS["GSK-2011",DATUM["Geodezicheskaya_Sistema_Koordinat_2011",SPHEROID["GSK-2011",6378136.5,298.2564151]],PRIMEM["Greenwich",0.0],UNIT["Degree",0.0174532925199433]],PROJECTION["Gauss_Kruger"],PARAMETER["False_Easting",4500000.0],PARAMETER["False_Northing",0.0],PARAMETER["Central_Meridian",21.0],PARAMETER["Scale_Factor",1.0],PARAMETER["Latitude_Of_Origin",0.0],UNIT["Meter",1.0]]"#,
        [
            (
                21.22,
                54.82,
                4514141.651862772,
                6077214.075227955,
                21.219999999999995,
                54.819999999999965,
            ),
            (
                21.62,
                55.12,
                4539557.58872845,
                6110764.354767358,
                21.619999999999994,
                55.11999999999994,
            ),
        ],
    ),
    (
        "EPSG:28355",
        r#"PROJCS["GDA_1994_MGA_Zone_55",GEOGCS["GCS_GDA_1994",DATUM["D_GDA_1994",SPHEROID["GRS_1980",6378137.0,298.257222101]],PRIMEM["Greenwich",0.0],UNIT["Degree",0.0174532925199433]],PROJECTION["Transverse_Mercator"],PARAMETER["False_Easting",500000.0],PARAMETER["False_Northing",10000000.0],PARAMETER["Central_Meridian",147.0],PARAMETER["Scale_Factor",0.9996],PARAMETER["Latitude_Of_Origin",0.0],UNIT["Meter",1.0]]"#,
        [
            (
                147.005,
                -30.06,
                500481.9480697148,
                6674566.120553608,
                147.005,
                -30.06,
            ),
            (
                147.405,
                -29.76,
                539155.1271819925,
                6707739.416562103,
                147.405,
                -29.759999999999994,
            ),
        ],
    ),
    (
        "EPSG:21500",
        r#"PROJCS["Belge_Lambert_1950",GEOGCS["GCS_Belge_1950_Brussels",DATUM["Reseau_National_Belge_1950_(Brussels)",SPHEROID["International_1924",6378388.0,297.0]],PRIMEM["Brussels",4.367975],UNIT["Degree",0.0174532925199433]],PROJECTION["Lambert_Conformal_Conic"],PARAMETER["False_Easting",150000.0],PARAMETER["False_Northing",5400000.0],PARAMETER["Central_Meridian",0.0],PARAMETER["Standard_Parallel_1",49.8333333333333],PARAMETER["Standard_Parallel_2",51.1666666666667],PARAMETER["Latitude_Of_Origin",90.0],UNIT["Meter",1.0]]"#,
        [
            (
                4.45,
                50.505,
                155818.89775084096,
                132543.23547713924,
                4.45,
                50.50500000000002,
            ),
            (
                4.85,
                50.805,
                183978.24109566584,
                166021.92765150685,
                4.85,
                50.805,
            ),
        ],
    ),
];

#[test]
fn esri_text_on_a_datum_with_a_null_or_helmert_operation_answers_like_pyproj() {
    let mut failures = Vec::new();
    for (name, wkt, points) in ESRI_CASES {
        for (lon, lat, x, y, back_lon, back_lat) in points {
            match transform_xy("EPSG:4326", wkt, *lon, *lat) {
                Err(e) => failures.push(format!("{name} forward at {lon} {lat}: {e}")),
                Ok((gx, gy)) if (gx - x).abs() > 0.01 || (gy - y).abs() > 0.01 => {
                    failures.push(format!("{name} forward: {gx} {gy} vs {x} {y}"));
                }
                Ok(_) => {}
            }
            match transform_xy(wkt, "EPSG:4326", *x, *y) {
                Err(e) => failures.push(format!("{name} inverse at {x} {y}: {e}")),
                Ok((bx, by)) if (bx - back_lon).abs() > 1e-7 || (by - back_lat).abs() > 1e-7 => {
                    failures.push(format!(
                        "{name} inverse: {bx} {by} vs {back_lon} {back_lat}"
                    ));
                }
                Ok(_) => {}
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// A projected CRS on a non-Greenwich prime meridian whose datum PROJ shifts is refused from its
/// text (it was answered 55 to 316 m off, or after C1 refused for another reason): NTF (Paris)
/// Lambert zone II, CH1903 (Bern) LV03C, Makassar (Jakarta) NEIEZ and MGI (Ferro) Austria West.
/// The texts are `pyproj.CRS.from_epsg(code).to_wkt("WKT1_ESRI")` without the identifier, the
/// point is the centre of the area of use. Giving the same CRS by its EPSG code works.
const PRIME_MERIDIAN_CASES: &[(&str, &str, f64, f64)] = &[
    (
        "EPSG:27572",
        r#"PROJCS["NTF_Paris_Lambert_Zone_II",GEOGCS["GCS_NTF_Paris",DATUM["Nouvelle_Triangulation_Francaise_(Paris)",SPHEROID["Clarke_1880_IGN",6378249.2,293.466021293627]],PRIMEM["Paris",2.33722917],UNIT["Grad",0.0157079632679489]],PROJECTION["Lambert_Conformal_Conic"],PARAMETER["False_Easting",600000.0],PARAMETER["False_Northing",2200000.0],PARAMETER["Central_Meridian",0.0],PARAMETER["Standard_Parallel_1",52.0],PARAMETER["Scale_Factor",0.99987742],PARAMETER["Latitude_Of_Origin",52.0],UNIT["Meter",1.0]]"#,
        549826.6306522982,
        2192990.7121902495,
    ),
    (
        "EPSG:21780",
        r#"PROJCS["Bern_1898_Bern_LV03C",GEOGCS["GCS_Bern_1898_Bern",DATUM["CH1903_(Bern)",SPHEROID["Bessel_1841",6377397.155,299.1528128]],PRIMEM["Bern",7.43958333333333],UNIT["Degree",0.0174532925199433]],PROJECTION["Hotine_Oblique_Mercator_Azimuth_Center"],PARAMETER["False_Easting",0.0],PARAMETER["False_Northing",0.0],PARAMETER["Scale_Factor",1.0],PARAMETER["Azimuth",90.0],PARAMETER["Longitude_Of_Center",0.0],PARAMETER["Latitude_Of_Center",46.9524055555556],UNIT["Meter",1.0]]"#,
        60019.11185791911,
        -15383.832160137368,
    ),
    (
        "EPSG:5329",
        r#"PROJCS["Gunung_Segara_Jakarta_NEIEZ",GEOGCS["GCS_Gunung_Segara_Jakarta",DATUM["Gunung_Segara_(Jakarta)",SPHEROID["Bessel_1841",6377397.155,299.1528128]],PRIMEM["Jakarta",106.807719444444],UNIT["Degree",0.0174532925199433]],PROJECTION["Mercator"],PARAMETER["False_Easting",3900000.0],PARAMETER["False_Northing",900000.0],PARAMETER["Central_Meridian",3.19228055555556],PARAMETER["Standard_Parallel_1",4.45405154589748],UNIT["Meter",1.0]]"#,
        4655117.850513344,
        902715.2093693333,
    ),
    (
        "EPSG:31281",
        r#"PROJCS["Austria_West_Zone",GEOGCS["GCS_MGI_Ferro",DATUM["Militar-Geographische_Institut_(Ferro)",SPHEROID["Bessel_1841",6377397.155,299.1528128]],PRIMEM["Ferro",-17.6666666666667],UNIT["Degree",0.0174532925199433]],PROJECTION["Transverse_Mercator"],PARAMETER["False_Easting",0.0],PARAMETER["False_Northing",0.0],PARAMETER["Central_Meridian",28.0],PARAMETER["Scale_Factor",1.0],PARAMETER["Latitude_Of_Origin",0.0],UNIT["Meter",1.0]]"#,
        26674.954644996207,
        5227959.912575018,
    ),
];

#[test]
fn esri_text_on_a_shifted_datum_with_another_prime_meridian_is_refused() {
    for (code, wkt, x, y) in PRIME_MERIDIAN_CASES {
        let err = transform_xy(wkt, "EPSG:4326", *x, *y)
            .expect_err(&format!("{code} from text must be refused"));
        assert!(
            err.starts_with("unsupported WKT CRS") && err.contains("prime meridian"),
            "{code}: {err}"
        );
        assert!(
            transform_xy(code, "EPSG:4326", *x, *y).is_ok(),
            "{code} by code"
        );
    }
}
