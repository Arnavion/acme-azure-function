pub const RFC3339_MILLISECONDS: &[time::format_description::FormatItem<'_>] = &[
	time::format_description::FormatItem::Component(time::format_description::Component::CalendarYearFullStandardRange(time::format_description::modifier::CalendarYearFullStandardRange::default())),
	time::format_description::FormatItem::StringLiteral("-"),
	time::format_description::FormatItem::Component(time::format_description::Component::MonthNumerical(time::format_description::modifier::MonthNumerical::default())),
	time::format_description::FormatItem::StringLiteral("-"),
	time::format_description::FormatItem::Component(time::format_description::Component::Day(time::format_description::modifier::Day::default())),
	time::format_description::FormatItem::StringLiteral("T"),
	time::format_description::FormatItem::Component(time::format_description::Component::Hour24(time::format_description::modifier::Hour24::default())),
	time::format_description::FormatItem::StringLiteral(":"),
	time::format_description::FormatItem::Component(time::format_description::Component::Minute(time::format_description::modifier::Minute::default())),
	time::format_description::FormatItem::StringLiteral(":"),
	time::format_description::FormatItem::Component(time::format_description::Component::Second(time::format_description::modifier::Second::default())),
	time::format_description::FormatItem::StringLiteral("."),
	time::format_description::FormatItem::Component(time::format_description::Component::Subsecond({
		let mut subsecond = time::format_description::modifier::Subsecond::default();
		subsecond.digits = time::format_description::modifier::SubsecondDigits::Three;
		subsecond
	})),
	time::format_description::FormatItem::StringLiteral("Z"),
];
