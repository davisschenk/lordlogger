#![feature(never_type)]

#[macro_use]
extern crate postgres;
#[macro_use]
extern crate postgres_derive;

use lordserial::{parser::Lord, Field, Packet};
use postgres::{types::to_sql_checked, Client, NoTls};
use serialport;

pub type Error = Box<dyn std::error::Error + Sync + Send>;

#[derive(Debug)]
struct Vector3f {
    x: f32,
    y: f32,
    z: f32,
}

impl Vector3f {
    fn extract(field: &Field) -> Result<Self, Error> {
        Ok(Self {
            x: field.extract::<f32>(0)?,
            y: field.extract::<f32>(4)?,
            z: field.extract::<f32>(8)?,
        })
    }
}

#[derive(Debug)]
struct Quaternion {
    q0: f32,
    q1: f32,
    q2: f32,
    q3: f32,
}

impl Quaternion {
    fn extract(field: &Field) -> Result<Self, Error> {
        Ok(Self {
            q0: field.extract::<f32>(0)?,
            q1: field.extract::<f32>(4)?,
            q2: field.extract::<f32>(8)?,
            q3: field.extract::<f32>(12)?,
        })
    }
}

#[derive(Debug)]
struct ImuData {
    accel: Vector3f,
    gyro: Vector3f,
    mag: Vector3f,
    baro: f32,
    delta_theta: Vector3f,
    delta_velocity: Vector3f,
    quat: Quaternion,
    euler_angles: Vector3f,
    tow: f64,
    week: i16,
}

impl ImuData {
    fn new(packet: &Packet) -> Result<Self, Error> {
        Ok(ImuData {
            accel: Vector3f::extract(packet.payload.get_field(0x04).unwrap())?,
            gyro: Vector3f::extract(packet.payload.get_field(0x05).unwrap())?,
            mag: Vector3f::extract(packet.payload.get_field(0x06).unwrap())?,
            baro: packet.payload.get_field(0x17).unwrap().extract::<f32>(0)?,
            delta_theta: Vector3f::extract(packet.payload.get_field(0x07).unwrap())?,
            delta_velocity: Vector3f::extract(packet.payload.get_field(0x08).unwrap())?,
            quat: Quaternion::extract(packet.payload.get_field(0x0A).unwrap())?,
            euler_angles: Vector3f::extract(packet.payload.get_field(0x0C).unwrap())?,
            tow: packet.payload.get_field(0x12).unwrap().extract(0)?,
            week: packet.payload.get_field(0x12).unwrap().extract(8)?,
        })
    }
}

fn setup_psql(c: &mut Client) -> Result<(), Error> {
    c.batch_execute(
        "
        CREATE TABLE IF NOT EXISTS imu_data (
            id SERIAL PRIMARY KEY,
            accel real3d NOT NULL,
            gyro real3d NOT NULL,
            mag real3d NOT NULL,
            baro real NOT NULL,
            delta_theta real3d NOT NULL,
            delta_velocity real3d NOT NULL,
            quat quaternion NOT NULL,
            euler_angles real3d NOT NULL,
            tow double precision NOT NULL,
            week smallint NOT NULL
        );

        CREATE TABLE IF NOT EXISTS gnss_data (
            id SERIAL PRIMARY KEY,
            
            latitude double precision NOT NULL,
            longitude double precision NOT NULL,
            ellipsoid_alt double precision NOT NULL,
            msl_alt double precision NOT NULL,
            horizontal_accuracy real NOT NULL,
            vertical_accuracy real NOT NULL,
            llh_flags smallint NOT NULL,

            ecefp_x double precision NOT NULL,
            ecefp_y double precision NOT NULL,
            ecefp_z double precision NOT NULL,
            ecefp_accuracy real NOT NULL,
            ecefp_flags smallint NOT NULL,

            ned_north real NOT NULL,
            ned_east real NOT NULL,
            ned_down real NOT NULL,
            ned_speed real NOT NULL,
            ned_ground_speed real NOT NULL,
            ned_heading real NOT NULL,
            ned_speed_accuracy real NOT NULL,
            ned_heading_accuracy real NOT NULL,
            ned_flags smallint NOT NULL,

            ecefv_x real NOT NULL,
            ecefv_y real NOT NULL,
            ecefv_z real NOT NULL,
            ecefv_accuracy real NOT NULL,
            ecefv_flags smallint NOT NULL,

            gdop real NOT NULL,
            pdop real NOT NULL,
            hdop real NOT NULL,
            vdop real NOT NULL,
            tdop real NOT NULL,
            ndop real NOT NULL,
            edop real NOT NULL,
            dop_flags smallint NOT NULL,

            tow double precision NOT NULL,
            week smallint NOT NULL,
            time_flags smallint NOT NULL,

            fix_type smallint NOT NULL,
            svs smallint NOT NULL,
            fix_flags smallint NOT NULL,
            fix_valid smallint NOT NULL
        );
    ",
    )?;

    Ok(())
}

fn setup_lord(lord: &mut Lord) -> Result<(), Error> {
    lord.set_imu_format(
        0x01,
        vec![
            (0x04, 50),
            (0x05, 50),
            (0x06, 50),
            (0x17, 50),
            (0x07, 50),
            (0x08, 50),
            (0x0A, 50),
            (0x0C, 50),
            (0x12, 50),
        ],
    )?;

    lord.set_gnss_format(
        0x01,
        vec![
            (0x03, 4),
            (0x04, 4),
            (0x05, 4),
            (0x06, 4),
            (0x07, 4),
            (0x09, 4),
            (0x0B, 4),
        ],
    )?;

    Ok(())
}

fn main() -> Result<!, Error> {
    let mut pg_client = Client::connect("postgres://lord:lord@localhost/lord", NoTls)?;
    setup_psql(&mut pg_client)?;

    let serial = serialport::new("/dev/ttyACM0", 115200)
        .open()
        .unwrap_or_else(|e| {
            eprintln!("Failed to open. Error: {}", e);
            ::std::process::exit(0);
        });

    let mut lord = Lord::new(serial);
    lord.start();
    setup_lord(&mut lord)?;

    loop {
        if let Some(packet) = lord.get_data() {
            match packet.header.descriptor {
                0x80 => {
                    println!("IMU DATA");
                    let data = ImuData::new(&packet)?;
                    pg_client.execute(
                        "
                    INSERT INTO imu_data (
                        accel,
                        gyro,
                        mag,
                        baro,
                        delta_theta,
                        delta_velocity,
                        quat,
                        euler_angles,
                        tow,
                        week
                    ) VALUES (
                        ROW($1, $2, $3),
                        ROW($4, $5, $6),
                        ROW($7, $8, $9),
                        $10,
                        ROW($11, $12, $13),
                        ROW($14, $15, $16),
                        ROW($17, $18, $19, $20),
                        ROW($21, $22, $23),
                        $24,
                        $25
                    );
                ",
                        &[
                            &data.accel.x,
                            &data.accel.y,
                            &data.accel.z,
                            &data.gyro.x,
                            &data.gyro.y,
                            &data.gyro.z,
                            &data.mag.x,
                            &data.mag.y,
                            &data.mag.z,
                            &data.baro,
                            &data.delta_theta.x,
                            &data.delta_theta.y,
                            &data.delta_theta.z,
                            &data.delta_velocity.x,
                            &data.delta_velocity.y,
                            &data.delta_velocity.z,
                            &data.quat.q0,
                            &data.quat.q1,
                            &data.quat.q2,
                            &data.quat.q3,
                            &data.euler_angles.x,
                            &data.euler_angles.y,
                            &data.euler_angles.z,
                            &data.tow,
                            &data.week,
                        ],
                    )?;
                }
                0x81 => {
                    println!("GNSS DATA");
                    pg_client.execute(
                        "
                        INSERT INTO gnss_data(
                            latitude, longitude, ellipsoid_alt, msl_alt, horizontal_accuracy, vertical_accuracy, llh_flags, ecefp_x, ecefp_y, ecefp_z, ecefp_accuracy, ecefp_flags, ned_north, ned_east, ned_down, ned_speed, ned_ground_speed, ned_heading, ned_speed_accuracy, ned_heading_accuracy, ned_flags, ecefv_x, ecefv_y, ecefv_z, ecefv_accuracy, ecefv_flags, gdop, pdop, hdop, vdop, tdop, ndop, edop, dop_flags, tow, week, time_flags, fix_type, svs, fix_flags, fix_valid)
                            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23, $24, $25, $26, $27, $28, $29, $30, $31, $32, $33, $34, $35, $36, $37, $38, $39, $40, $41);
                    ",
                        &[
                            &packet.payload.get_field(0x03).unwrap().extract::<f64>(0)?,
                            &packet.payload.get_field(0x03).unwrap().extract::<f64>(8)?,
                            &packet.payload.get_field(0x03).unwrap().extract::<f64>(16)?,
                            &packet.payload.get_field(0x03).unwrap().extract::<f64>(24)?,
                            &packet.payload.get_field(0x03).unwrap().extract::<f32>(32)?,
                            &packet.payload.get_field(0x03).unwrap().extract::<f32>(36)?,
                            &packet.payload.get_field(0x03).unwrap().extract::<i16>(40)?,

                            &packet.payload.get_field(0x04).unwrap().extract::<f64>(0)?,
                            &packet.payload.get_field(0x04).unwrap().extract::<f64>(8)?,
                            &packet.payload.get_field(0x04).unwrap().extract::<f64>(16)?,
                            &packet.payload.get_field(0x04).unwrap().extract::<f32>(24)?,
                            &packet.payload.get_field(0x04).unwrap().extract::<i16>(28)?,

                            &packet.payload.get_field(0x05).unwrap().extract::<f32>(0)?,
                            &packet.payload.get_field(0x05).unwrap().extract::<f32>(4)?,
                            &packet.payload.get_field(0x05).unwrap().extract::<f32>(8)?,
                            &packet.payload.get_field(0x05).unwrap().extract::<f32>(12)?,
                            &packet.payload.get_field(0x05).unwrap().extract::<f32>(16)?,
                            &packet.payload.get_field(0x05).unwrap().extract::<f32>(20)?,
                            &packet.payload.get_field(0x05).unwrap().extract::<f32>(24)?,
                            &packet.payload.get_field(0x05).unwrap().extract::<f32>(28)?,
                            &packet.payload.get_field(0x05).unwrap().extract::<i16>(32)?,

                            &packet.payload.get_field(0x06).unwrap().extract::<f32>(0)?,
                            &packet.payload.get_field(0x06).unwrap().extract::<f32>(4)?,
                            &packet.payload.get_field(0x06).unwrap().extract::<f32>(8)?,
                            &packet.payload.get_field(0x06).unwrap().extract::<f32>(12)?,
                            &packet.payload.get_field(0x06).unwrap().extract::<i16>(16)?,

                            &packet.payload.get_field(0x07).unwrap().extract::<f32>(0)?,
                            &packet.payload.get_field(0x07).unwrap().extract::<f32>(4)?,
                            &packet.payload.get_field(0x07).unwrap().extract::<f32>(8)?,
                            &packet.payload.get_field(0x07).unwrap().extract::<f32>(12)?,
                            &packet.payload.get_field(0x07).unwrap().extract::<f32>(16)?,
                            &packet.payload.get_field(0x07).unwrap().extract::<f32>(20)?,
                            &packet.payload.get_field(0x07).unwrap().extract::<f32>(24)?,
                            &packet.payload.get_field(0x07).unwrap().extract::<i16>(28)?,

                            &packet.payload.get_field(0x09).unwrap().extract::<f64>(0)?,
                            &packet.payload.get_field(0x09).unwrap().extract::<i16>(8)?,
                            &packet.payload.get_field(0x09).unwrap().extract::<i16>(10)?,

                            &(packet.payload.get_field(0x0B).unwrap().extract::<i8>(0)? as i16),
                            &(packet.payload.get_field(0x0B).unwrap().extract::<i8>(1)? as i16),
                            &packet.payload.get_field(0x0B).unwrap().extract::<i16>(2)?,
                            &packet.payload.get_field(0x0B).unwrap().extract::<i16>(4)?,
                        ],
                    )?;
                }
                _ => (),
            }
        }
    }
}
