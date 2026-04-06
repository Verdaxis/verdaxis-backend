#!/usr/bin/env python3
"""
Seed script for producer_projects table in Verdaxis maritime fuel trading platform.
Populates with 48 realistic green fuel projects worldwide.
Idempotent: Deletes all GENA/IEA/IRENA projects before inserting.
"""

import psycopg2
from psycopg2.extras import execute_values
from datetime import datetime
from uuid import uuid4

conn = psycopg2.connect(
    host="localhost", port=5432, dbname="verdaxis",
    user="postgres", password="Tealtent477"
)
conn.autocommit = False
cursor = conn.cursor()

PROJECTS = [
    ("European Green Hydrogen Hub", "E-Methanol", 100, "Denmark", "Southern Jutland", "SRID=4326;POINT(8.45 55.47)", 2022, "2022-06-15", "OPERATIONAL", "GENA Registry", "GENA-MeOH-001", "Renewable electricity + CO2", "Electrolysis + CO2 hydrogenation", 8.2),
    ("Ørsted/Maersk Green Fuels", "E-Methanol", 50, "Denmark", "Southern Jutland", "SRID=4326;POINT(9.15 55.07)", 2027, None, "UNDER_CONSTRUCTION", "GENA Registry", "GENA-MeOH-002", "Renewable electricity + CO2", "Electrolysis + CO2 hydrogenation", 7.8),
    ("HIF Global eMethanol", "E-Methanol", 180, "Uruguay", "Río Negro", "SRID=4326;POINT(-57.35 -32.32)", 2026, None, "UNDER_CONSTRUCTION", "GENA Registry", "GENA-MeOH-003", "Renewable electricity + CO2", "Electrolysis + CO2 hydrogenation", 6.5),
    ("ABEL Energy Green Methanol", "E-Methanol", 75, "Australia", "Tasmania", "SRID=4326;POINT(146.85 -41.15)", 2029, None, "ANNOUNCED", "GENA Registry", "GENA-MeOH-004", "Renewable electricity + CO2", "Electrolysis + CO2 hydrogenation", 5.9),
    ("Goldwind Green Methanol", "E-Methanol", 200, "China", "Qinghai", "SRID=4326;POINT(97.37 37.37)", 2027, None, "UNDER_CONSTRUCTION", "GENA Registry", "GENA-MeOH-005", "Renewable electricity + CO2", "Electrolysis + CO2 hydrogenation", 4.2),
    ("CAC China Green Methanol", "E-Methanol", 110, "China", "Jilin", "SRID=4326;POINT(126.55 43.84)", 2023, "2023-03-20", "OPERATIONAL", "GENA Registry", "GENA-MeOH-006", "Renewable electricity + CO2", "Electrolysis + CO2 hydrogenation", 7.1),
    ("GIDARA Energy Green Methanol", "E-Methanol", 90, "Netherlands", "North Holland", "SRID=4326;POINT(4.9 52.37)", 2027, None, "UNDER_CONSTRUCTION", "GENA Registry", "GENA-MeOH-007", "Renewable electricity + CO2", "Electrolysis + CO2 hydrogenation", 6.8),
    ("Proman eMethanol Caribbean", "E-Methanol", 150, "Trinidad and Tobago", "Trinidad", "SRID=4326;POINT(-61.25 10.45)", 2029, None, "ANNOUNCED", "GENA Registry", "GENA-MeOH-008", "Renewable electricity + CO2", "Electrolysis + CO2 hydrogenation", 5.4),
    ("Carbon Recycling International", "E-Methanol", 4, "Iceland", "Reykjanes", "SRID=4326;POINT(-22.35 63.87)", 2012, "2012-10-08", "OPERATIONAL", "GENA Registry", "GENA-MeOH-009", "Renewable electricity + CO2", "Electrolysis + CO2 hydrogenation", 3.1),
    ("Liquid Wind FlagshipONE", "E-Methanol", 50, "Sweden", "Västernorrland", "SRID=4326;POINT(18.71 63.29)", 2026, None, "UNDER_CONSTRUCTION", "GENA Registry", "GENA-MeOH-010", "Renewable electricity + CO2", "Electrolysis + CO2 hydrogenation", 4.7),
    ("Southern Green Hydrogen Methanol", "E-Methanol", 300, "Australia", "Queensland", "SRID=4326;POINT(151.27 -23.85)", 2030, None, "ANNOUNCED", "GENA Registry", "GENA-MeOH-011", "Renewable electricity + CO2", "Electrolysis + CO2 hydrogenation", 5.1),
    ("Green Fuels Nordic Luleå", "E-Methanol", 100, "Sweden", "Norrbotten", "SRID=4326;POINT(22.15 65.58)", 2029, None, "ANNOUNCED", "GENA Registry", "GENA-MeOH-012", "Renewable electricity + CO2", "Electrolysis + CO2 hydrogenation", 4.3),
    ("REintegrate Green Methanol", "E-Methanol", 32, "Denmark", "North Jutland", "SRID=4326;POINT(9.92 57.05)", 2021, "2021-09-10", "OPERATIONAL", "GENA Registry", "GENA-MeOH-013", "Renewable electricity + CO2", "Electrolysis + CO2 hydrogenation", 7.5),
    ("WasteFuel Green Methanol", "E-Methanol", 50, "Philippines", "Zambales", "SRID=4326;POINT(120.28 14.83)", 2028, None, "ANNOUNCED", "GENA Registry", "GENA-MeOH-014", "Renewable electricity + CO2", "Electrolysis + CO2 hydrogenation", 6.2),
    ("Samsung C&T eMethanol", "E-Methanol", 80, "South Korea", "Chungcheongnam-do", "SRID=4326;POINT(126.61 36.33)", 2029, None, "ANNOUNCED", "GENA Registry", "GENA-MeOH-015", "Renewable electricity + CO2", "Electrolysis + CO2 hydrogenation", 6.9),
    ("OCI Global eMethanol", "E-Methanol", 125, "United States", "Texas", "SRID=4326;POINT(-94.1 30.08)", 2027, None, "UNDER_CONSTRUCTION", "GENA Registry", "GENA-MeOH-016", "Renewable electricity + CO2", "Electrolysis + CO2 hydrogenation", 9.1),
    ("NEOM Green Hydrogen", "Green Ammonia", 1200, "Saudi Arabia", "Tabuk", "SRID=4326;POINT(36.5 27.9)", 2026, None, "UNDER_CONSTRUCTION", "IRENA", "GENA-NH3-001", "Renewable electricity + N2", "Electrolysis + Haber-Bosch", 3.1),
    ("Yara HEGRA", "Green Ammonia", 500, "Norway", "Telemark", "SRID=4326;POINT(9.66 59.12)", 2028, None, "ANNOUNCED", "IRENA", "GENA-NH3-002", "Renewable electricity + N2", "Electrolysis + Haber-Bosch", 2.4),
    ("Asian Renewable Energy Hub", "Green Ammonia", 9000, "Australia", "Western Australia", "SRID=4326;POINT(119.5 -20.7)", 2030, None, "ANNOUNCED", "IRENA", "GENA-NH3-003", "Renewable electricity + N2", "Electrolysis + Haber-Bosch", 1.8),
    ("ACME Green Ammonia", "Green Ammonia", 100, "Oman", "Muscat", "SRID=4326;POINT(57.0 23.6)", 2027, None, "UNDER_CONSTRUCTION", "IRENA", "GENA-NH3-004", "Renewable electricity + N2", "Electrolysis + Haber-Bosch", 3.5),
    ("Fortescue Green Ammonia", "Green Ammonia", 250, "Australia", "Queensland", "SRID=4326;POINT(151.27 -23.85)", 2029, None, "ANNOUNCED", "IRENA", "GENA-NH3-005", "Renewable electricity + N2", "Electrolysis + Haber-Bosch", 2.9),
    ("CF Industries Green Ammonia", "Green Ammonia", 200, "United States", "Louisiana", "SRID=4326;POINT(-91.0 30.1)", 2029, None, "ANNOUNCED", "IRENA", "GENA-NH3-006", "Renewable electricity + N2", "Electrolysis + Haber-Bosch", 3.6),
    ("Enaex Green Ammonia", "Green Ammonia", 80, "Chile", "Antofagasta", "SRID=4326;POINT(-70.45 -23.1)", 2029, None, "ANNOUNCED", "IRENA", "GENA-NH3-007", "Renewable electricity + N2", "Electrolysis + Haber-Bosch", 1.9),
    ("Iberdrola/Fertiberia Green Ammonia", "Green Ammonia", 200, "Spain", "Ciudad Real", "SRID=4326;POINT(-4.11 38.69)", 2027, None, "UNDER_CONSTRUCTION", "IRENA", "GENA-NH3-008", "Renewable electricity + N2", "Electrolysis + Haber-Bosch", 2.1),
    ("ENGIE Green Ammonia", "Green Ammonia", 1000, "Australia", "Western Australia", "SRID=4326;POINT(115.86 -31.95)", 2030, None, "ANNOUNCED", "IRENA", "GENA-NH3-009", "Renewable electricity + N2", "Electrolysis + Haber-Bosch", 1.7),
    ("Scatec Green Ammonia", "Green Ammonia", 300, "Yemen", "Aden", "SRID=4326;POINT(45.03 12.8)", 2030, None, "ANNOUNCED", "IRENA", "GENA-NH3-010", "Renewable electricity + N2", "Electrolysis + Haber-Bosch", 3.2),
    ("TotalEnergies Green Ammonia", "Green Ammonia", 600, "Oman", "Dhofar", "SRID=4326;POINT(57.7 19.67)", 2030, None, "ANNOUNCED", "IRENA", "GENA-NH3-011", "Renewable electricity + N2", "Electrolysis + Haber-Bosch", 2.8),
    ("H2 Green Steel Ammonia", "Green Ammonia", 150, "Sweden", "Norrbotten", "SRID=4326;POINT(21.69 66.6)", 2029, None, "ANNOUNCED", "IRENA", "GENA-NH3-012", "Renewable electricity + N2", "Electrolysis + Haber-Bosch", 2.2),
    ("HyDeal Ambition", "Green Hydrogen", 3600, "Spain", "Basque Country", "SRID=4326;POINT(-2.7 43.0)", 2030, None, "ANNOUNCED", "IEA Hydrogen Projects DB", "GENA-H2-001", "Renewable electricity", "PEM/Alkaline electrolysis", 0.4),
    ("NortH2", "Green Hydrogen", 800, "Netherlands", "Groningen", "SRID=4326;POINT(6.83 53.44)", 2030, None, "ANNOUNCED", "IEA Hydrogen Projects DB", "GENA-H2-002", "Renewable electricity", "PEM/Alkaline electrolysis", 0.5),
    ("AquaVentus", "Green Hydrogen", 300, "Germany", "Schleswig-Holstein", "SRID=4326;POINT(7.89 54.18)", 2030, None, "ANNOUNCED", "IEA Hydrogen Projects DB", "GENA-H2-003", "Renewable electricity", "PEM/Alkaline electrolysis", 0.6),
    ("H2 Magallanes", "Green Hydrogen", 120, "Chile", "Magallanes", "SRID=4326;POINT(-70.91 -53.15)", 2029, None, "ANNOUNCED", "IEA Hydrogen Projects DB", "GENA-H2-004", "Renewable electricity", "PEM/Alkaline electrolysis", 0.3),
    ("HyDeal Espana", "Green Hydrogen", 400, "Spain", "Asturias", "SRID=4326;POINT(-5.85 43.36)", 2027, None, "UNDER_CONSTRUCTION", "IEA Hydrogen Projects DB", "GENA-H2-005", "Renewable electricity", "PEM/Alkaline electrolysis", 0.7),
    ("Power-to-X Mauritania", "Green Hydrogen", 500, "Mauritania", "Nouakchott", "SRID=4326;POINT(-15.98 18.09)", 2030, None, "ANNOUNCED", "IEA Hydrogen Projects DB", "GENA-H2-006", "Renewable electricity", "PEM/Alkaline electrolysis", 0.8),
    ("Sun Cable H2", "Green Hydrogen", 200, "Australia", "Northern Territory", "SRID=4326;POINT(130.84 -12.46)", 2029, None, "ANNOUNCED", "IEA Hydrogen Projects DB", "GENA-H2-007", "Renewable electricity", "PEM/Alkaline electrolysis", 0.2),
    ("Masdar Green H2", "Green Hydrogen", 100, "United Arab Emirates", "Abu Dhabi", "SRID=4326;POINT(54.37 24.45)", 2027, None, "UNDER_CONSTRUCTION", "IEA Hydrogen Projects DB", "GENA-H2-008", "Renewable electricity", "PEM/Alkaline electrolysis", 0.9),
    ("Neste Rotterdam", "HVO/Biofuel", 1360, "Netherlands", "South Holland", "SRID=4326;POINT(4.47 51.92)", 2021, "2021-03-15", "OPERATIONAL", "GENA Registry", "GENA-BIO-001", "Waste oils and fats", "Hydrotreating/HEFA", 15.0),
    ("TotalEnergies La Mede", "HVO/Biofuel", 500, "France", "Provence-Alpes-Cote Azur", "SRID=4326;POINT(5.13 43.4)", 2019, "2019-06-20", "OPERATIONAL", "GENA Registry", "GENA-BIO-002", "Waste oils and fats", "Hydrotreating/HEFA", 16.2),
    ("Eni Gela Biorefinery", "HVO/Biofuel", 750, "Italy", "Sicily", "SRID=4326;POINT(14.25 37.07)", 2019, "2019-04-10", "OPERATIONAL", "GENA Registry", "GENA-BIO-003", "Waste oils and fats", "Hydrotreating/HEFA", 17.1),
    ("UPM Lappeenranta", "HVO/Biofuel", 130, "Finland", "South Karelia", "SRID=4326;POINT(28.19 61.06)", 2020, "2020-11-12", "OPERATIONAL", "GENA Registry", "GENA-BIO-004", "Waste oils and fats", "Hydrotreating/HEFA", 14.8),
    ("Preem HVO Lysekil", "HVO/Biofuel", 600, "Sweden", "Västra Götaland", "SRID=4326;POINT(11.44 58.27)", 2026, None, "UNDER_CONSTRUCTION", "GENA Registry", "GENA-BIO-005", "Waste oils and fats", "Hydrotreating/HEFA", 15.7),
    ("Renewable Energy Group Geismar", "HVO/Biofuel", 340, "United States", "Louisiana", "SRID=4326;POINT(-91.0 30.22)", 2018, "2018-08-05", "OPERATIONAL", "GENA Registry", "GENA-BIO-006", "Waste oils and fats", "Hydrotreating/HEFA", 16.5),
    ("Diamond Green Diesel Norco", "HVO/Biofuel", 1200, "United States", "Louisiana", "SRID=4326;POINT(-90.36 29.99)", 2020, "2020-10-25", "OPERATIONAL", "GENA Registry", "GENA-BIO-007", "Waste oils and fats", "Hydrotreating/HEFA", 15.4),
    ("Shell Pernis Biorefinery", "HVO/Biofuel", 820, "Netherlands", "South Holland", "SRID=4326;POINT(4.38 51.88)", 2027, None, "UNDER_CONSTRUCTION", "GENA Registry", "GENA-BIO-008", "Waste oils and fats", "Hydrotreating/HEFA", 16.8),
    ("Pavilion Energy Singapore", "LNG", 1500, "Singapore", "Singapore", "SRID=4326;POINT(103.82 1.26)", 2014, "2014-03-20", "OPERATIONAL", "GENA Registry", "GENA-LNG-001", "Natural gas", "Liquefaction", 56.0),
    ("Equinor Hammerfest LNG", "LNG", 5700, "Norway", "Finnmark", "SRID=4326;POINT(23.68 70.66)", 2007, "2007-09-12", "OPERATIONAL", "GENA Registry", "GENA-LNG-002", "Natural gas", "Liquefaction", 57.2),
    ("Shell Prelude FLNG", "LNG", 3600, "Australia", "Western Australia", "SRID=4326;POINT(124.0 -13.75)", 2018, "2018-12-15", "OPERATIONAL", "GENA Registry", "GENA-LNG-003", "Natural gas", "Liquefaction", 58.5),
    ("NextDecade Rio Grande LNG", "LNG", 11000, "United States", "Texas", "SRID=4326;POINT(-97.4 25.97)", 2027, None, "UNDER_CONSTRUCTION", "GENA Registry", "GENA-LNG-004", "Natural gas", "Liquefaction", 55.0),
]

try:
    print("Deleting existing GENA/IEA/IRENA sourced projects...")
    cursor.execute(
        "DELETE FROM producer_projects WHERE data_source LIKE 'GENA%' OR data_source LIKE 'IEA%' OR data_source = 'IRENA'"
    )
    deleted = cursor.rowcount
    print(f"Deleted {deleted} existing projects.\n")

    print(f"Inserting {len(PROJECTS)} new projects...")
    now = datetime.utcnow()
    
    data = []
    for proj in PROJECTS:
        name, fuel_type, capacity, country, region, location, cod_year, cod_date, status, source, gena_id, feedstock, technology, ci = proj
        data.append((
            str(uuid4()), name, fuel_type, capacity, country, region, location,
            cod_date, cod_year, status, source, gena_id, None,
            feedstock, technology, ci, None, now, now,
        ))

    execute_values(cursor, """
        INSERT INTO producer_projects (
            id, name, fuel_type, capacity_kt_per_year,
            country, region, location, cod_date, cod_year, status,
            data_source, gena_project_id, organization_id,
            feedstock, technology, carbon_intensity_gco2_mj, notes,
            created_at, updated_at
        ) VALUES %s
    """, data)

    inserted = cursor.rowcount
    print(f"Inserted {inserted} projects.")

    conn.commit()
    print(f"\nSuccessfully committed {inserted} projects to database.")

    cursor.execute("SELECT COUNT(*) FROM producer_projects")
    total = cursor.fetchone()[0]
    print(f"Total projects in table: {total}")

except Exception as e:
    conn.rollback()
    print(f"Error: {e}")
    import traceback
    traceback.print_exc()
    exit(1)

finally:
    cursor.close()
    conn.close()

