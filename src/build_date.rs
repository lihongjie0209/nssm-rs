pub fn from_unix_seconds(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::from_unix_seconds;

    #[test]
    fn converts_unix_dates_across_leap_and_century_boundaries() {
        assert_eq!(from_unix_seconds(0), "1970-01-01");
        assert_eq!(from_unix_seconds(951_782_400), "2000-02-29");
        assert_eq!(from_unix_seconds(1_709_164_800), "2024-02-29");
        assert_eq!(from_unix_seconds(2_147_483_647), "2038-01-19");
    }
}
