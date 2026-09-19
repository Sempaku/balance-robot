using System.Windows.Controls;
using ScottPlot.WPF;

namespace BalanceBot.Desktop.Views;

public partial class BlackBoxView : UserControl
{
    public WpfPlot PlotBlackBoxControl => PlotBlackBox;

    public BlackBoxView()
    {
        InitializeComponent();
    }
}
